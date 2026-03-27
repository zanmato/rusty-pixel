use axum::{
  extract::{Path, State},
  http::{StatusCode, header},
  response::IntoResponse,
};
use libvips::{VipsImage, ops};
use tracing::error;

use crate::http::AppState;
use crate::image_modifier;

use crate::http::error::AppError;

#[utoipa::path(
  get,
  path = "/scale/{options}/{uri}",
  params(
    ("options" = String, description = "Image transformation options (e.g., 's40x30-m10-rh200')"),
    ("uri" = String, description = "URI/path to the source image")
  ),
  responses(
    (status = 200, description = "Successfully transformed image", content_type = "image/jpeg"),
    (status = 404, description = "Image not found"),
    (status = 500, description = "Internal server error")
  )
)]
pub async fn scale(
  Path((options, uri)): Path<(String, String)>,
  State(state): State<AppState>,
) -> impl IntoResponse {
  // Read image from storage using the provided uri
  let data = match state.storage_client.download_object(&uri).await {
    Ok(data) => data,
    Err(_) => {
      return AppError::NotFound.into_response();
    }
  };

  // Run the image transformation in a thread from the thread pool
  let (send, recv) = tokio::sync::oneshot::channel();
  rayon::spawn(move || {
    // Parse options and create modifiers
    let modifiers = parse_options(&options);
    if modifiers.is_empty() {
      let _ = send.send(Err("no valid options provided".to_owned()));
      return;
    }

    let mut output_image = match VipsImage::new_from_buffer(&data, "") {
      Ok(img) => img,
      Err(e) => {
        let _ = send.send(Err(format!("failed to load image: {}", e)));
        return;
      }
    };

    for opt in modifiers {
      match opt.apply(&output_image) {
        Err(e) => {
          let _ = send.send(Err(e.to_string()));
          return;
        }
        Ok(Some(m)) => output_image = m,
        Ok(None) => {}
      }
    }

    match ops::jpegsave_buffer_with_opts(
      &output_image,
      &ops::JpegsaveBufferOptions {
        q: 80,
        background: vec![255.0, 255.0, 255.0],
        profile: "sRGB".to_owned(),
        ..ops::JpegsaveBufferOptions::default()
      },
    ) {
      Ok(buffer) => {
        let _ = send.send(Ok(buffer));
      }
      Err(e) => {
        let _ = send.send(Err(e.to_string()));
      }
    }

    // Ensure data buffer outlives VipsImage C references
    drop(data);
  });

  let headers = [(header::CONTENT_TYPE, "image/jpeg")];

  match recv.await {
    Ok(Ok(image_data)) => (StatusCode::OK, headers, image_data).into_response(),
    Ok(Err(e)) => {
      error!(
        "failed to transform image: {} {}",
        e,
        state.vips_app.error_buffer().unwrap_or("")
      );
      (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
    }
    Err(e) => {
      error!("failed to receive from image processing task: {}", e);
      (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
    }
  }
}

fn parse_options(option_string: &str) -> Vec<Box<dyn image_modifier::ImageModifier>> {
  let options: Vec<&str> = option_string.split('-').collect();
  let mut opts = Vec::new();

  let eval_options: Vec<image_modifier::ImageModifierEvaluator> = vec![
    image_modifier::orientation::OrientationModifier::evaluate,
    image_modifier::blackandwhite::BlackAndWhiteModifier::evaluate,
    image_modifier::trim::TrimModifier::evaluate,
    image_modifier::scale::ScaleModifier::evaluate,
    image_modifier::resize::ResizeModifier::evaluate,
  ];

  for opt in &options {
    for eval in eval_options.iter() {
      if let Some(o) = eval(opt, &options) {
        opts.push(o);
        break;
      }
    }
  }

  opts
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_options_scale_and_margin() {
    let opts = parse_options("s400x300-m10");
    assert_eq!(opts.len(), 1); // scale modifier (margin is consumed by scale)
  }

  #[test]
  fn parse_options_multiple_modifiers() {
    let opts = parse_options("bw-olandscape-s400x400-m20");
    assert_eq!(opts.len(), 3); // blackandwhite, orientation, scale
  }

  #[test]
  fn parse_options_resize_height() {
    let opts = parse_options("rh200");
    assert_eq!(opts.len(), 1);
  }

  #[test]
  fn parse_options_resize_width() {
    let opts = parse_options("rw300");
    assert_eq!(opts.len(), 1);
  }

  #[test]
  fn parse_options_empty_string() {
    let opts = parse_options("");
    assert_eq!(opts.len(), 0);
  }

  #[test]
  fn parse_options_invalid() {
    let opts = parse_options("invalid-xyz-123");
    assert_eq!(opts.len(), 0);
  }

  #[test]
  fn parse_options_trim() {
    let opts = parse_options("tr-s200x200");
    assert_eq!(opts.len(), 2); // trim + scale
  }
}
