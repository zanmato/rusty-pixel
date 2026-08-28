use anyhow::anyhow;
use axum::{
  Router,
  extract::{DefaultBodyLimit, Request, State},
  http::{HeaderName, StatusCode},
  middleware::{self, Next},
  response::{IntoResponse, Response},
  routing::{get, post},
};
use std::future::ready;
use std::{path::Path, sync::Arc};
use tokio::signal;
use tokio::time::Duration;
use tower_http::{
  catch_panic::CatchPanicLayer,
  request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
  timeout::TimeoutLayer,
  trace::{self, TraceLayer},
};
use tracing::{Level, info_span};
use utoipa::OpenApi;
use utoipa_redoc::{Redoc, Servable};

use crate::config::{Config, StorageType};
use crate::image_processing::{
  EnvironmentImage, ImageConditions, ImageConfiguration, ImageProcessingRequest, ProcessedImage,
};
use anyhow::Result;
use libvips::VipsApp;

mod error;
mod local_storage;
pub mod metrics;
mod process_image;
mod s3;
mod scale_image;
mod storage;

#[derive(OpenApi)]
#[openapi(
  paths(
    process_image::process_image,
    scale_image::scale
  ),
  components(
    schemas(ImageProcessingRequest, ImageConfiguration, ImageConditions, EnvironmentImage, ProcessedImage)
  ),
  modifiers(&SecurityAddon),
  info(
    title = "Rusty Pixel API",
    version = "0.1.3",
    description = "Image proxy service that applies real-time image transformations using libvips"
  )
)]
struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
  fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
    if let Some(components) = openapi.components.as_mut() {
      components.add_security_scheme(
        "api_key",
        utoipa::openapi::security::SecurityScheme::ApiKey(
          utoipa::openapi::security::ApiKey::Header(utoipa::openapi::security::ApiKeyValue::new(
            "X-API-Key",
          )),
        ),
      );
    }
  }
}

#[derive(Clone)]
struct AppState {
  storage_client: Arc<dyn storage::Storage>,
  vips_app: Arc<VipsApp>,
  api_key: String,
  upload_concurrency: usize,
}

const DEFAULT_UPLOAD_CONCURRENCY: usize = 4;

const X_API_KEY: &str = "X-API-Key";

async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
  let auth_header = req
    .headers()
    .get(X_API_KEY)
    .and_then(|header| header.to_str().ok());

  let auth_header = if let Some(auth_header) = auth_header {
    auth_header
  } else {
    return StatusCode::UNAUTHORIZED.into_response();
  };

  if !auth_header.eq(&state.api_key) {
    return StatusCode::UNAUTHORIZED.into_response();
  }

  next.run(req).await
}

/// The routers that make up the service. The main router serves the API and
/// the metrics router serves Prometheus metrics and health probes on a
/// separate listener so they are never exposed publicly.
pub struct App {
  pub router: Router,
  pub metrics: Router,
}

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

pub fn bootstrap(cfg: &Config) -> Result<App> {
  // Init vips
  let vips_app = Arc::new(VipsApp::new("rusty-pixel", false).expect("Cannot initialize libvips"));
  // Set number of threads in libvips's threadpool
  vips_app.concurrency_set(cfg.app.vips_concurrency);

  // Disable vips cache
  vips_app.cache_set_max_mem(0);
  vips_app.cache_set_max(0);
  vips_app.cache_set_max_files(0);

  // Size the worker pool. The global pool can only be built once per process,
  // which matters for tests that bootstrap more than once.
  let worker_threads = cfg
    .app
    .worker_threads
    .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
  if let Err(e) = rayon::ThreadPoolBuilder::new()
    .num_threads(worker_threads)
    .thread_name(|i| format!("image-worker-{i}"))
    .build_global()
  {
    tracing::warn!(
      "worker pool already initialised, keeping existing size: {}",
      e
    );
  }

  // Init storage client
  let storage_client: Arc<dyn storage::Storage> = match cfg.storage.storage_type {
    StorageType::Local => {
      let path = Path::new(&cfg.storage.local.as_ref().unwrap().path).to_path_buf();
      Arc::new(local_storage::Client::new(path))
    }
    StorageType::S3 => {
      let storage_config = match &cfg.storage.s3 {
        Some(s3) => s3,
        None => return Err(anyhow!("S3 storage config is missing")),
      };

      let cred = aws_sdk_s3::config::Credentials::new(
        storage_config.access_key_id.clone(),
        storage_config.secret_access_key.clone(),
        None,
        None,
        "loaded-from-custom-env",
      );

      let s3_config = aws_sdk_s3::config::Builder::new()
        .endpoint_url(storage_config.endpoint.clone())
        .credentials_provider(cred)
        .region(aws_sdk_s3::config::Region::new(
          storage_config.region.clone(),
        ))
        .force_path_style(storage_config.force_path_style) // apply bucketname as path param instead of pre-domain
        .behavior_version_latest()
        .build();

      let client = aws_sdk_s3::Client::from_conf(s3_config);
      Arc::new(s3::Client::new(
        client,
        storage_config.bucket.as_str(),
        storage_config.base_url.as_str(),
      ))
    }
  };

  // App state
  let state = AppState {
    storage_client,
    vips_app,
    api_key: cfg.app.api_key.clone(),
    upload_concurrency: cfg
      .app
      .upload_concurrency
      .unwrap_or(DEFAULT_UPLOAD_CONCURRENCY)
      .max(1),
  };

  // Routing
  let public_app = Router::new().route("/scale/{options}/{*uri}", get(scale_image::scale));

  let private_app = Router::new()
    .route("/api/v1/process-image", post(process_image::process_image))
    .layer((
      DefaultBodyLimit::max(cfg.app.max_body_size_mb * 1000 * 1000),
      middleware::from_fn_with_state(state.clone(), auth),
    ));

  let mut app = Router::new()
    .merge(private_app)
    .merge(public_app)
    .with_state(state);

  // Conditionally add OpenAPI routes if enabled
  if cfg.app.enable_openapi.unwrap_or(false) {
    app = app
      .merge(Redoc::with_url("/redoc", ApiDoc::openapi()))
      .route(
        "/api-docs/openapi.json",
        get(|| async { axum::Json(ApiDoc::openapi()) }),
      );
  }

  let router = app.layer((
    SetRequestIdLayer::new(X_REQUEST_ID, MakeRequestUuid),
    middleware::from_fn(metrics::track),
    TraceLayer::new_for_http()
      .make_span_with(|req: &Request| {
        let request_id = req
          .headers()
          .get(X_REQUEST_ID)
          .and_then(|v| v.to_str().ok())
          .unwrap_or("");
        info_span!(
          "request",
          method = %req.method(),
          uri = %req.uri(),
          request_id,
        )
      })
      .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
    PropagateRequestIdLayer::new(X_REQUEST_ID),
    TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(60)),
    CatchPanicLayer::new(),
  ));

  Ok(App {
    router,
    metrics: metrics_app(),
  })
}

pub async fn serve(router: Router, listen: &str) {
  // Start HTTP server
  let listener = tokio::net::TcpListener::bind(listen)
    .await
    .expect("failed to bind to address");
  axum::serve(listener, router)
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("error running HTTP server");
}

async fn healthz() -> &'static str {
  "pong"
}

async fn shutdown_signal() {
  let ctrl_c = async {
    signal::ctrl_c()
      .await
      .expect("failed to install Ctrl+C handler");
  };

  #[cfg(unix)]
  let terminate = async {
    signal::unix::signal(signal::unix::SignalKind::terminate())
      .expect("failed to install signal handler")
      .recv()
      .await;
  };

  #[cfg(not(unix))]
  let terminate = std::future::pending::<()>();

  tokio::select! {
      _ = ctrl_c => {},
      _ = terminate => {},
  }
}

pub async fn serve_metrics(router: Router, listen: &str) {
  let listener = tokio::net::TcpListener::bind(listen)
    .await
    .expect("failed to bind to address");
  axum::serve(listener, router)
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("error running metrics HTTP server");
}

fn metrics_app() -> Router {
  let recorder_handle = metrics::setup_recorder();
  Router::new()
    .route("/metrics", get(move || ready(recorder_handle.render())))
    .route("/healthz", get(healthz))
}
