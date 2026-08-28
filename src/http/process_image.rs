use crate::image_modifier::environment::EnvironmentOptions;
use crate::image_processing::pipeline::{self, Environment, Pipeline, PipelineError};
use crate::image_processing::{
  ImageProcessingRequest, ProcessImageForm, ProcessedImage, UploadImage,
};

use axum::{
  Json,
  body::Bytes,
  extract::{self, State},
};
use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};
use tokio::task::JoinSet;
use tracing::{Span, error, info};

use crate::http::AppState;
use crate::http::error::AppError;
use crate::http::metrics::{self, Timer};

#[utoipa::path(
  post,
  path = "/api/v1/process-image",
  request_body(content = ProcessImageForm, content_type = "multipart/form-data"),
  responses(
    (status = 200, description = "Successfully processed images", body = [ProcessedImage]),
    (status = 400, description = "Bad request - invalid input"),
    (status = 401, description = "Unauthorized - invalid API key"),
    (status = 404, description = "Not found - environment image not found"),
    (status = 500, description = "Internal server error")
  ),
  security(("api_key" = []))
)]
pub async fn process_image(
  State(state): State<AppState>,
  mut multipart: extract::Multipart,
) -> Result<axum::Json<Vec<ProcessedImage>>, AppError> {
  let mut processing_request: Option<ImageProcessingRequest> = None;
  let mut uploaded_image: Option<Bytes> = None;

  while let Some(field) = multipart
    .next_field()
    .await
    .map_err(|e| AppError::BadRequest(e.to_string()))?
  {
    let name = field.name().unwrap_or("");

    match name {
      "image" => {
        uploaded_image = Some(
          field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?,
        );
      }
      "details" => {
        let bytes = field
          .bytes()
          .await
          .map_err(|e| AppError::BadRequest(e.to_string()))?;
        processing_request =
          serde_json::from_slice(&bytes).map_err(|e| AppError::BadRequest(e.to_string()))?;
      }
      _ => {}
    }
  }

  let (processing_request, data) = match (processing_request, uploaded_image) {
    (Some(pr), Some(ui)) => (pr, ui),
    _ => return Err(AppError::BadRequest("missing image or details".to_owned())),
  };

  if let Some(config) = processing_request
    .configurations
    .iter()
    .find(|c| c.size > state.limits.max_dimension || c.size < 1)
  {
    return Err(AppError::BadRequest(format!(
      "configuration {} size {} is outside 1..={}",
      config.id, config.size, state.limits.max_dimension
    )));
  }

  // Flag the worker when this future is dropped, which happens when the client
  // disconnects or the timeout layer gives up on the request.
  let cancel = Arc::new(AtomicBool::new(false));
  let _cancel_guard = CancelOnDrop(cancel.clone());

  // Read the header on the pool, libvips is not called from async threads
  let source = {
    let data = data.clone();
    let vips = state.vips_app.clone();
    let span = Span::current();
    run_on_pool(move || {
      let _guard = span.enter();
      pipeline::source_info(&data, &vips)
    })
    .await?
  };

  if let Some(min_size) = processing_request.min_size
    && source.width < min_size
    && source.height < min_size
  {
    return Err(AppError::BadRequest("image too small".to_owned()));
  }

  let environment_image_conf = if source.is_portrait() {
    processing_request.portrait_environment_image.as_ref()
  } else {
    processing_request.landscape_environment_image.as_ref()
  };

  // Download and decode the environment image once for all configurations
  let environment = if let Some(env_conf) = environment_image_conf {
    let object_data = state
      .storage_client
      .download_object(&env_conf.path)
      .await
      .map_err(|_| AppError::NotFound)?;

    let opts = EnvironmentOptions {
      width: env_conf.width,
      height: env_conf.height,
      x: env_conf.x,
      y: env_conf.y,
      margin_percent: env_conf.margin_percent,
    };

    let vips = state.vips_app.clone();
    Some(run_on_pool(move || Environment::decode(&object_data, opts, &vips)).await?)
  } else {
    None
  };

  ::metrics::histogram!(metrics::CONFIGURATIONS)
    .record(processing_request.configurations.len() as f64);

  let (send, recv) = tokio::sync::oneshot::channel();
  let (tx, mut rx) = tokio::sync::mpsc::channel(processing_request.configurations.len().max(1) * 2);

  let pipeline = Pipeline {
    data,
    source,
    request: processing_request,
    environment,
    vips: state.vips_app.clone(),
    cancel,
  };

  // Run the image transformation on the thread pool
  let span = Span::current();
  let queued = Timer::start();
  rayon::spawn(move || {
    let _guard = span.enter();
    queued.observe(metrics::QUEUE_WAIT);
    let processing = Timer::start();

    let result = pipeline.run(tx);

    processing.observe(metrics::PROCESS_DURATION);
    let _ = send.send(result);
  });

  // Upload outputs as they are produced, a few at a time
  let mut uploads: JoinSet<UploadResult> = JoinSet::new();
  let mut processed_images: Vec<(usize, ProcessedImage)> = Vec::new();

  let mut finish =
    |result: Option<Result<UploadResult, tokio::task::JoinError>>| -> Result<(), AppError> {
      match result {
        Some(Ok(Ok(image))) => {
          processed_images.push(image);
          Ok(())
        }
        Some(Ok(Err(e))) => Err(e),
        Some(Err(e)) => Err(AppError::InternalServerError(format!(
          "upload task failed: {}",
          e
        ))),
        None => Ok(()),
      }
    };

  while let Some(img) = rx.recv().await {
    if uploads.len() >= state.upload_concurrency {
      finish(uploads.join_next().await)?;
    }

    let storage = state.storage_client.clone();
    let span = Span::current();
    uploads.spawn(async move {
      let _guard = span.enter();
      upload(storage, img).await
    });
  }

  while !uploads.is_empty() {
    finish(uploads.join_next().await)?;
  }

  match recv.await {
    Ok(Ok(())) => {}
    Ok(Err(PipelineError::InvalidImage(msg))) => return Err(AppError::BadRequest(msg)),
    Ok(Err(PipelineError::Cancelled)) => {
      info!("image processing cancelled");
      return Err(AppError::InternalServerError("cancelled".to_owned()));
    }
    Ok(Err(PipelineError::Failed(msg))) => {
      error!("image processing failed: {}", msg);
      return Err(AppError::InternalServerError(msg));
    }
    Err(recv_err) => {
      error!(
        "image processing task panicked or was dropped: {}",
        recv_err
      );
      return Err(AppError::InternalServerError(recv_err.to_string()));
    }
  }

  processed_images.sort_by_key(|(order, _)| *order);

  info!(images = processed_images.len(), "processed image");

  Ok(Json(
    processed_images
      .into_iter()
      .map(|(_, image)| image)
      .collect(),
  ))
}

type UploadResult = Result<(usize, ProcessedImage), AppError>;

async fn upload(
  storage: Arc<dyn crate::http::storage::Storage>,
  img: UploadImage,
) -> Result<(usize, ProcessedImage), AppError> {
  let timer = Timer::start();
  let upload_res = match storage.upload_object(img.data, &img.path, &img.mime).await {
    Ok(r) => r,
    Err(e) => {
      ::metrics::counter!(metrics::UPLOAD_ERRORS).increment(1);
      error!("failed to upload image {}: {:#}", img.path, e);
      return Err(AppError::InternalServerError(e.to_string()));
    }
  };
  timer.observe(metrics::UPLOAD_DURATION);

  Ok((
    img.order,
    ProcessedImage {
      id: img.id,
      path: img.path,
      hash: upload_res.etag,
      size: upload_res.size,
      url: upload_res.url,
      mime: img.mime,
      alternative_to: img.alternative_to,
      width: img.width,
      height: img.height,
    },
  ))
}

/// Run a short blocking job on the rayon pool and await its result.
async fn run_on_pool<T, F>(job: F) -> Result<T, AppError>
where
  T: Send + 'static,
  F: FnOnce() -> Result<T, PipelineError> + Send + 'static,
{
  let (send, recv) = tokio::sync::oneshot::channel();
  rayon::spawn(move || {
    let _ = send.send(job());
  });

  match recv.await {
    Ok(Ok(value)) => Ok(value),
    Ok(Err(PipelineError::InvalidImage(msg))) => Err(AppError::BadRequest(msg)),
    Ok(Err(e)) => Err(AppError::InternalServerError(e.to_string())),
    Err(_) => Err(AppError::InternalServerError("worker dropped".to_owned())),
  }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
  fn drop(&mut self) {
    self.0.store(true, Ordering::Relaxed);
  }
}
