use anyhow::anyhow;
use axum::{
  extract::{DefaultBodyLimit, MatchedPath, Request, State},
  http::StatusCode,
  middleware::{self, Next},
  response::{IntoResponse, Response},
  routing::{get, post},
  Router,
};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::future::ready;
use std::{path::Path, sync::Arc};
use tokio::signal;
use tokio::time::{Duration, Instant};
use tower_http::{
  catch_panic::CatchPanicLayer,
  timeout::TimeoutLayer,
  trace::{self, TraceLayer},
};
use tracing::Level;
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
mod process_image;
mod s3;
mod scale_image;
pub mod storage;

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
  storage_client: Arc<Box<dyn storage::Storage>>,
  vips_app: Arc<VipsApp>,
  api_key: String,
}

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

pub fn bootstrap(cfg: &Config) -> Result<Router> {
  // Init vips
  let vips_app = Arc::new(VipsApp::new("rusty-pixel", false).expect("Cannot initialize libvips"));
  // Set number of threads in libvips's threadpool
  vips_app.concurrency_set(cfg.app.vips_concurrency);

  // Disable vips cache
  vips_app.cache_set_max_mem(0);
  vips_app.cache_set_max(0);
  vips_app.cache_set_max_files(0);

  // Init storage client
  let storage_client: Arc<Box<dyn storage::Storage>> = match cfg.storage.storage_type {
    StorageType::Local => {
      let path = Path::new(&cfg.storage.local.as_ref().unwrap().path).to_path_buf();
      Arc::new(Box::new(local_storage::Client::new(path)))
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
      Arc::new(Box::new(s3::Client::new(
        client,
        storage_config.bucket.as_str(),
        storage_config.base_url.as_str(),
        storage_config.original_base_url.as_deref(),
      )))
    }
  };

  // App state
  let state = AppState {
    storage_client,
    vips_app,
    api_key: cfg.app.api_key.clone(),
  };

  // Routing
  let public_app = Router::new().route("/scale/:options/*uri", get(scale_image::scale));

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
      .merge(Redoc::with_url(
        "/redoc",
        serde_json::to_value(ApiDoc::openapi()).unwrap(),
      ))
      .route(
        "/api-docs/openapi.json",
        get(|| async { axum::Json(ApiDoc::openapi()) }),
      );
  }

  let app = app.layer((
    middleware::from_fn(track_metrics),
    TraceLayer::new_for_http()
      .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
      .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
    TimeoutLayer::new(Duration::from_secs(60)),
    CatchPanicLayer::new(),
  ));

  Ok(app)
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

pub async fn serve_metrics(listen: &str) {
  let app = metrics_app();

  let listener = tokio::net::TcpListener::bind(listen)
    .await
    .expect("failed to bind to address");
  axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("error running metrics HTTP server");
}

fn metrics_app() -> Router {
  let recorder_handle = setup_metrics_recorder();
  Router::new()
    .route("/metrics", get(move || ready(recorder_handle.render())))
    .route("/healthz", get(healthz))
}

fn setup_metrics_recorder() -> PrometheusHandle {
  const EXPONENTIAL_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
  ];

  PrometheusBuilder::new()
    .set_buckets_for_metric(
      Matcher::Full("http_requests_duration_seconds".to_string()),
      EXPONENTIAL_SECONDS,
    )
    .unwrap()
    .install_recorder()
    .unwrap()
}

async fn track_metrics(req: Request, next: Next) -> impl IntoResponse {
  let start = Instant::now();
  let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
    matched_path.as_str().to_owned()
  } else {
    req.uri().path().to_owned()
  };
  let method = req.method().clone();

  let response = next.run(req).await;

  let latency = start.elapsed().as_secs_f64();
  let status = response.status().as_u16().to_string();

  let labels = [
    ("method", method.to_string()),
    ("path", path),
    ("status", status),
  ];

  metrics::counter!("http_requests_total", &labels).increment(1);
  metrics::histogram!("http_requests_duration_seconds", &labels).record(latency);

  response
}
