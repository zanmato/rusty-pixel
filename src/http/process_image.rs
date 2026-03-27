use crate::image_modifier;
use crate::image_processing::{
  self, ImageProcessingRequest, ProcessImageForm, ProcessedImage, UploadImage,
};

use anyhow::anyhow;
use axum::{
  Json,
  extract::{self, State},
};
use libvips::{VipsImage, ops};
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

use crate::http::AppState;
use crate::http::error::AppError;

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
  let mut uploaded_image: Option<axum::body::Bytes> = None;

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

  let (processing_request, uploaded_image) = match (processing_request, uploaded_image) {
    (Some(pr), Some(ui)) => (pr, ui),
    _ => return Err(AppError::BadRequest("missing image or details".to_owned())),
  };
  let data = Arc::new(uploaded_image.to_vec());

  let (image_portrait_sender, image_portrait_recv) = tokio::sync::oneshot::channel();

  let orientation_data = data.clone();
  rayon::spawn(move || {
    let image = match VipsImage::new_from_buffer(&orientation_data, "") {
      Ok(i) => i,
      Err(e) => {
        let _ = image_portrait_sender.send(Err(AppError::BadRequest(e.to_string())));
        return;
      }
    };

    let _ = image_portrait_sender.send(Ok((image.get_width(), image.get_height())));
  });

  let image_size = image_portrait_recv
    .await
    .map_err(|_| AppError::InternalServerError("orientation detection failed".into()))??;
  let image_portrait = image_size.0 < image_size.1;

  if let Some(min_size) = processing_request.min_size {
    if image_size.0 < min_size && image_size.1 < min_size {
      return Err(AppError::BadRequest("image too small".to_owned()));
    }
  }

  let environment_image_conf = if image_portrait {
    processing_request.portrait_environment_image.as_ref()
  } else {
    processing_request.landscape_environment_image.as_ref()
  };

  // Download the environment image from storage if there is one
  let (environment_image, environment_image_opts) = if let Some(env_conf) = environment_image_conf {
    let object_data = state
      .storage_client
      .download_object(&env_conf.path)
      .await
      .map_err(|_| AppError::NotFound)?;

    let opts = image_modifier::environment::EnvironmentOptions {
      width: env_conf.width,
      height: env_conf.height,
      x: env_conf.x,
      y: env_conf.y,
      margin_percent: env_conf.margin_percent,
    };

    (Some(Arc::new(object_data)), Some(opts))
  } else {
    (None, None)
  };

  let (send, recv) = tokio::sync::oneshot::channel();
  let (tx, mut rx) = tokio::sync::mpsc::channel(processing_request.configurations.len().max(1));

  // Run the image transformation in a thread from the thread pool
  rayon::spawn(move || {
    // Decode the image once and reuse across all configurations
    let source_image = match VipsImage::new_from_buffer(&data, "") {
      Ok(i) => i,
      Err(e) => {
        let _ = send.send(Err(anyhow!("failed to create image from buffer: {}", e)));
        return;
      }
    };

    let loader = match source_image.get_string("vips-loader") {
      Ok(l) => l,
      Err(e) => {
        let _ = send.send(Err(anyhow!("failed to get vips-loader metadata: {}", e)));
        return;
      }
    };

    for config in processing_request.configurations {
      // Pass the image as is
      if config.conditions.allow_vector && loader == "svgload_buffer" {
        if let Err(e) = tx.blocking_send(UploadImage {
          path: format!("{}.svg", &config.path),
          mime: "image/svg+xml".to_string(),
          id: config.id.clone(),
          data: data.clone(),
          alternative_to: None,
        }) {
          let _ = send.send(Err(anyhow!("failed to send image: {}", e)));
          return;
        }

        let _ = send.send(Ok(()));
        return;
      }

      let alternative_possible = image_processing::alternative_possible(loader);

      // Create a lightweight copy of the decoded image for this configuration
      let mut output_image = match ops::copy(&source_image) {
        Ok(img) => img,
        Err(e) => {
          let _ = send.send(Err(anyhow!("failed to copy source image: {}", e)));
          return;
        }
      };

      // Build a vector of modifiers to apply to the image
      let mut modifiers: Vec<Box<dyn image_modifier::ImageModifier>> = Vec::new();

      if config.conditions.black_and_white {
        modifiers.push(Box::new(
          image_modifier::blackandwhite::BlackAndWhiteModifier,
        ));
      }

      if config.conditions.trim {
        modifiers.push(Box::new(image_modifier::trim::TrimModifier::new(vec![
          255.0, 255.0, 255.0,
        ])));
      }

      // If we are trimming, don't crop the resulting image
      modifiers.push(Box::new(image_modifier::scale::ScaleModifier::new(
        config.aspect,
        config.margin_percent,
        Some(config.size),
        !config.conditions.trim,
      )));

      if config.conditions.use_environment_image {
        if let (Some(env_img), Some(env_opts)) = (&environment_image, &environment_image_opts) {
          modifiers.push(Box::new(
            image_modifier::environment::EnvironmentModifier::new(
              env_img.clone(),
              env_opts.clone(),
            ),
          ));
        }
      }

      for opt in modifiers {
        match opt.apply(&output_image) {
          Err(e) => {
            let _ = send.send(Err(anyhow!("failed to apply modifier: {}", e)));
            return;
          }
          Ok(Some(m)) => output_image = m,
          Ok(None) => {}
        }
      }

      // Save as png if the image is transparent
      let image_data = if config.conditions.transparent {
        match ops::pngsave_buffer_with_opts(
          &output_image,
          &ops::PngsaveBufferOptions {
            profile: "sRGB".to_owned(),
            ..ops::PngsaveBufferOptions::default()
          },
        ) {
          Ok(data) => Arc::new(data),
          Err(e) => {
            let _ = send.send(Err(anyhow!("failed to save image: {}", e)));
            return;
          }
        }
      } else {
        match ops::jpegsave_buffer_with_opts(
          &output_image,
          &ops::JpegsaveBufferOptions {
            q: config.quality,
            background: vec![255.0, 255.0, 255.0],
            profile: "sRGB".to_owned(),
            ..ops::JpegsaveBufferOptions::default()
          },
        ) {
          Ok(data) => Arc::new(data),
          Err(e) => {
            let _ = send.send(Err(anyhow!("failed to save image: {}", e)));
            return;
          }
        }
      };

      let (ext, mime) = if config.conditions.transparent {
        ("png", "image/png")
      } else {
        ("jpg", "image/jpeg")
      };

      // Pass the resulting image via the channel
      if let Err(e) = tx.blocking_send(UploadImage {
        path: format!("{}.{}", &config.path, ext),
        mime: mime.to_owned(),
        id: config.id.clone(),
        data: image_data,
        alternative_to: None,
      }) {
        let _ = send.send(Err(anyhow!("failed to send image: {}", e)));
        return;
      }

      // Generate an alternative format if possible
      if alternative_possible {
        let webp_data = match ops::webpsave_buffer_with_opts(
          &output_image,
          &ops::WebpsaveBufferOptions {
            q: config.quality,
            background: vec![255.0, 255.0, 255.0],
            profile: "sRGB".to_owned(),
            ..ops::WebpsaveBufferOptions::default()
          },
        ) {
          Ok(data) => Arc::new(data),
          Err(e) => {
            let _ = send.send(Err(anyhow!("failed to save image: {}", e)));
            return;
          }
        };

        let alternative_id = Uuid::new_v4();
        if let Err(e) = tx.blocking_send(UploadImage {
          path: format!("{}.webp", &config.path),
          id: alternative_id.into(),
          data: webp_data,
          mime: "image/webp".to_owned(),
          alternative_to: Some(config.id.clone()),
        }) {
          let _ = send.send(Err(anyhow!("failed to send image: {}", e)));
          return;
        }
      }
    }

    // Upload the given image as well
    if processing_request.save_original {
      let meta = image_processing::loader_to_mime_ext(loader);
      let _ = tx.blocking_send(UploadImage {
        path: format!("{}.{}", &processing_request.path, meta.1),
        id: processing_request.id.clone(),
        data,
        mime: meta.0.to_owned(),
        alternative_to: None,
      });
    }

    let _ = send.send(Ok(()));
  });

  let mut processed_images = Vec::new();
  while let Some(img) = rx.recv().await {
    // Upload image
    let data = match Arc::try_unwrap(img.data) {
      Ok(data) => data,
      Err(arc) => (*arc).clone(),
    };
    let upload_res = match state
      .storage_client
      .upload_object(data, &img.path, &img.mime)
      .await
    {
      Ok(r) => r,
      Err(e) => {
        error!("failed to upload image: {}", e);
        rx.close();
        return Err(AppError::InternalServerError(e.to_string()));
      }
    };

    processed_images.push(ProcessedImage {
      id: img.id,
      path: img.path,
      hash: upload_res.etag,
      size: upload_res.size,
      url: upload_res.url,
      mime: img.mime,
      alternative_to: img.alternative_to,
    });
  }

  if let Err(recv_err) = recv.await {
    error!("failed to receive: {}", recv_err);
    rx.close();
    return Err(AppError::InternalServerError(recv_err.to_string()));
  }

  rx.close();

  Ok(Json(processed_images))
}
