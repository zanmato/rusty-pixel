use crate::image_modifier;
use crate::image_processing::{self, ImageProcessingRequest, ProcessedImage, UploadImage};

use anyhow::anyhow;
use axum::{
  extract::{self, State},
  Json,
};
use libvips::{ops, VipsImage};
use std::sync::Arc;
use tracing::error;
use uuid::Uuid;

use crate::http::error::AppError;
use crate::http::AppState;

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
        uploaded_image = Some(field.bytes().await.map_err(|e| AppError::BadRequest(e.to_string()))?);
      }
      "details" => {
        let bytes = field.bytes().await.map_err(|e| AppError::BadRequest(e.to_string()))?;
        processing_request = serde_json::from_slice(&bytes)
          .map_err(|e| AppError::BadRequest(e.to_string()))?;
      }
      _ => {}
    }
  }

  if processing_request.is_none() || uploaded_image.is_none() {
    return Err(AppError::BadRequest("missing image or details".to_owned()));
  }

  let processing_request = processing_request.unwrap();
  let data = Arc::new(uploaded_image.unwrap().to_vec());

  let (image_portrait_sender, image_portrait_recv) = tokio::sync::oneshot::channel();

  let orientation_data = data.clone();
  rayon::spawn(move || {
    let orientation_data = orientation_data.clone();
    let image = match VipsImage::new_from_buffer(&orientation_data, "") {
      Ok(i) => i,
      Err(e) => {
        let _ = image_portrait_sender.send(Err(AppError::BadRequest(e.to_string())));
        return;
      }
    };

    image_portrait_sender
      .send(Ok((image.get_width(), image.get_height())))
      .unwrap();
  });

  let image_size = image_portrait_recv.await.unwrap()?;
  let image_portrait = image_size.0 < image_size.1;

  if let Some(min_size) = processing_request.min_size {
    if image_size.0 < min_size && image_size.1 < min_size {
      return Err(AppError::BadRequest("image too small".to_owned()));
    }
  }

  let environment_image_conf: Option<image_processing::EnvironmentImage> = match image_portrait {
    false => processing_request.landscape_environment_image.clone(),
    true => processing_request.portrait_environment_image.clone(),
  };

  let mut environment_image: Option<Arc<Vec<u8>>> = None;
  let mut environment_image_opts: Option<image_modifier::environment::EnvironmentOptions> = None;

  // Download the environment image from storage if there is one
  if environment_image_conf.is_some() {
    let env_image_conf = environment_image_conf.clone().unwrap();
    let object_data = state
      .storage_client
      .download_object(&env_image_conf.path)
      .await;
    if object_data.is_err() {
      return Err(AppError::NotFound);
    }

    let data = Arc::new(object_data.unwrap());

    environment_image = Some(data);

    environment_image_opts = Some(image_modifier::environment::EnvironmentOptions {
      width: env_image_conf.width,
      height: env_image_conf.height,
      x: env_image_conf.x,
      y: env_image_conf.y,
      margin_percent: env_image_conf.margin_percent,
    });
  }

  let (send, recv) = tokio::sync::oneshot::channel();
  let (tx, mut rx) = tokio::sync::mpsc::channel(processing_request.configurations.len().max(1));

  // Run the image transformation in a thread from the thread pool
  rayon::spawn(move || {
    for config in processing_request.configurations {
      let data = data.clone();
      let image = match VipsImage::new_from_buffer(&data, "") {
        Ok(i) => i,
        Err(e) => {
          let _ = send.send(Err(anyhow!("failed to create image from buffer: {}", e)));
          return;
        }
      };

      let mut output_image: VipsImage = image;

      let loader = match output_image.get_string("vips-loader") {
        Ok(l) => l,
        Err(e) => {
          let _ = send.send(Err(anyhow!("failed to get vips-loader metadata: {}", e)));
          return;
        }
      };

      // Pass the image as is
      if config.conditions.allow_vector && loader == "svgload_buffer" {
        if let Err(e) = tx.blocking_send(UploadImage {
          path: format!("{}.svg", &config.path),
          mime: "image/svg+xml".to_string(),
          id: config.id.clone(),
          data,
          alternative_to: None,
        }) {
          let _ = send.send(Err(anyhow!("failed to send image: {}", e)));
          return;
        }

        let _ = send.send(Ok(()));
        return;
      }

      let alternative_possible = image_processing::alternative_possible(loader);

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

      if config.conditions.use_environment_image && environment_image.is_some() {
        modifiers.push(Box::new(
          image_modifier::environment::EnvironmentModifier::new(
            environment_image.clone().unwrap(),
            environment_image_opts.clone().unwrap(),
          ),
        ));
      }

      for opt in modifiers {
        let modifier_res = opt.apply(&output_image);
        if let Err(e) = modifier_res {
          let _ = send.send(Err(anyhow!("failed to apply modifier: {}", e.to_string())));
          return;
        }

        if let Some(m) = modifier_res.unwrap() {
          output_image = m;
        }
      }

      // Save as png if the image is transparent
      let res = if config.conditions.transparent {
        ops::pngsave_buffer_with_opts(
          &output_image,
          &ops::PngsaveBufferOptions {
            profile: "sRGB".to_owned(),
            ..ops::PngsaveBufferOptions::default()
          },
        )
      } else {
        ops::jpegsave_buffer_with_opts(
          &output_image,
          &ops::JpegsaveBufferOptions {
            q: config.quality,
            background: vec![255.0, 255.0, 255.0],
            profile: "sRGB".to_owned(),
            ..ops::JpegsaveBufferOptions::default()
          },
        )
      };

      if let Err(e) = res {
        let _ = send.send(Err(anyhow!("failed to save image: {}", e.to_string())));
        return;
      }

      // Pass the resulting image via the channel
      if let Err(e) = tx.blocking_send(UploadImage {
        path: match config.conditions.transparent {
          true => format!("{}.png", &config.path),
          false => format!("{}.jpg", &config.path),
        },
        mime: match config.conditions.transparent {
          true => "image/png",
          false => "image/jpeg",
        }
        .to_owned(),
        id: config.id.clone(),
        data: Arc::new(res.unwrap()),
        alternative_to: None,
      }) {
        let _ = send.send(Err(anyhow!("failed to send image: {}", e)));
        return;
      }

      // Generate an alternative format if possible
      if alternative_possible {
        let res = ops::webpsave_buffer_with_opts(
          &output_image,
          &ops::WebpsaveBufferOptions {
            q: config.quality,
            background: vec![255.0, 255.0, 255.0],
            profile: "sRGB".to_owned(),
            ..ops::WebpsaveBufferOptions::default()
          },
        );

        if let Err(e) = res {
          let _ = send.send(Err(anyhow!("failed to save image: {}", e.to_string())));
          return;
        }

        let alternative_id = Uuid::new_v4();
        if let Err(e) = tx.blocking_send(UploadImage {
          path: format!("{}.webp", &config.path),
          id: alternative_id.into(),
          data: Arc::new(res.unwrap()),
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
      let data = data.clone();
      let image = match VipsImage::new_from_buffer(&data, "") {
        Ok(i) => i,
        Err(e) => {
          let _ = send.send(Err(anyhow!("failed to create image from buffer: {}", e)));
          return;
        }
      };
      let loader = match image.get_string("vips-loader") {
        Ok(l) => l,
        Err(e) => {
          let _ = send.send(Err(anyhow!("failed to get vips-loader metadata: {}", e)));
          return;
        }
      };

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
