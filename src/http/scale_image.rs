use axum::{
  extract::{Path, State},
  http::{StatusCode, header},
  response::IntoResponse,
};
use libvips::{VipsImage, ops};
use tracing::{Span, error};

use crate::http::AppState;
use crate::http::metrics::{self, Timer};
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
    (status = 400, description = "Invalid options"),
    (status = 404, description = "Image not found"),
    (status = 500, description = "Internal server error")
  )
)]
pub async fn scale(
  Path((options, uri)): Path<(String, String)>,
  State(state): State<AppState>,
) -> impl IntoResponse {
  // Parse options before touching storage so bad requests are cheap
  let modifiers = match parse_options(&options, state.limits.max_dimension) {
    Ok(modifiers) => modifiers,
    Err(e) => return AppError::BadRequest(e).into_response(),
  };

  // Read image from storage using the provided uri
  let data = match state.storage_client.download_object(&uri).await {
    Ok(data) => data,
    Err(_) => {
      return AppError::NotFound.into_response();
    }
  };

  // Run the image transformation in a thread from the thread pool
  let (send, recv) = tokio::sync::oneshot::channel();
  let span = Span::current();
  let queued = Timer::start();
  let vips = state.vips_app.clone();
  let quality = state.limits.scale_quality;
  rayon::spawn(move || {
    let _guard = span.enter();
    queued.observe(metrics::QUEUE_WAIT);
    let processing = Timer::start();

    let mut output_image = match VipsImage::new_from_buffer(&data, "") {
      Ok(img) => img,
      Err(e) => {
        let _ = send.send(Err(format!(
          "failed to load image: {} {}",
          e,
          vips.error_buffer().unwrap_or("")
        )));
        vips.error_clear();
        return;
      }
    };

    for opt in modifiers {
      match opt.apply(&output_image) {
        Err(e) => {
          let _ = send.send(Err(format!("{} {}", e, vips.error_buffer().unwrap_or(""))));
          vips.error_clear();
          return;
        }
        Ok(Some(m)) => output_image = m,
        Ok(None) => {}
      }
    }

    let output_image = match to_srgb(&output_image) {
      Ok(Some(converted)) => converted,
      Ok(None) => output_image,
      Err(e) => {
        let _ = send.send(Err(format!(
          "failed to convert to sRGB: {} {}",
          e,
          vips.error_buffer().unwrap_or("")
        )));
        vips.error_clear();
        return;
      }
    };

    let encode = Timer::start();
    match ops::jpegsave_buffer_with_opts(
      &output_image,
      &ops::JpegsaveBufferOptions {
        q: quality,
        optimize_coding: true,
        background: vec![255.0, 255.0, 255.0],
        keep: ops::ForeignKeep::None,
        ..ops::JpegsaveBufferOptions::default()
      },
    ) {
      Ok(buffer) => {
        encode.observe_with(metrics::ENCODE_DURATION, "format", "jpeg");
        ::metrics::histogram!(metrics::OUTPUT_BYTES, "format" => "jpeg")
          .record(buffer.len() as f64);
        processing.observe(metrics::PROCESS_DURATION);
        let _ = send.send(Ok(buffer));
      }
      Err(e) => {
        let _ = send.send(Err(format!(
          "failed to encode image: {} {}",
          e,
          vips.error_buffer().unwrap_or("")
        )));
        vips.error_clear();
      }
    }

    // Ensure data buffer outlives VipsImage C references
    drop(data);
  });

  match recv.await {
    Ok(Ok(image_data)) => {
      let headers = [
        (header::CONTENT_TYPE, "image/jpeg".to_owned()),
        (
          header::CACHE_CONTROL,
          state.limits.scale_cache_control.clone(),
        ),
      ];
      (StatusCode::OK, headers, image_data).into_response()
    }
    Ok(Err(e)) => {
      error!("failed to transform image: {}", e);
      (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
    }
    Err(e) => {
      error!("failed to receive from image processing task: {}", e);
      (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
    }
  }
}

/// Convert the image to sRGB before it is encoded with its metadata stripped.
/// Without this a wide gamut or CMYK source that no thumbnail modifier touched,
/// for example `bw` or `tr` on their own, would lose the profile that gives its
/// pixel values meaning and render with shifted colours.
fn to_srgb(img: &VipsImage) -> libvips::Result<Option<VipsImage>> {
  if img.get_as_string("icc-profile-data").is_ok() {
    return ops::icc_transform_with_opts(
      img,
      "srgb",
      &ops::IccTransformOptions {
        embedded: true,
        ..ops::IccTransformOptions::default()
      },
    )
    .map(Some);
  }

  match img.get_interpretation() {
    Ok(ops::Interpretation::Cmyk) => ops::colourspace(img, ops::Interpretation::Srgb).map(Some),
    _ => Ok(None),
  }
}

/// Parse the option string into modifiers. The whole request is rejected when
/// no option is valid or when any option would produce an output larger than
/// `max_dimension` on either side, which stops the public endpoint from being
/// used to burn CPU on huge upscales.
fn parse_options(
  option_string: &str,
  max_dimension: i32,
) -> Result<Vec<Box<dyn image_modifier::ImageModifier>>, String> {
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
        if o.max_output_dimension().is_some_and(|d| d > max_dimension) {
          return Err(format!(
            "option {} exceeds the maximum dimension of {}",
            opt, max_dimension
          ));
        }
        opts.push(o);
        break;
      }
    }
  }

  if opts.is_empty() {
    return Err("no valid options provided".to_owned());
  }

  Ok(opts)
}

#[cfg(test)]
mod tests {
  use super::*;

  const MAX: i32 = 4096;

  #[test]
  fn parse_options_scale_and_margin() {
    let opts = parse_options("s400x300-m10", MAX).unwrap();
    assert_eq!(opts.len(), 1); // scale modifier (margin is consumed by scale)
  }

  #[test]
  fn parse_options_multiple_modifiers() {
    let opts = parse_options("bw-olandscape-s400x400-m20", MAX).unwrap();
    assert_eq!(opts.len(), 3); // blackandwhite, orientation, scale
  }

  #[test]
  fn parse_options_resize_height() {
    let opts = parse_options("rh200", MAX).unwrap();
    assert_eq!(opts.len(), 1);
  }

  #[test]
  fn parse_options_resize_width() {
    let opts = parse_options("rw300", MAX).unwrap();
    assert_eq!(opts.len(), 1);
  }

  #[test]
  fn parse_options_resize_over_limit_is_rejected() {
    assert!(parse_options("rw5000", MAX).is_err());
  }

  #[test]
  fn parse_options_over_limit_rejects_whole_request() {
    // Valid options next to an oversized one must not be applied on their own
    assert!(parse_options("bw-rw5000", MAX).is_err());
  }

  #[test]
  fn parse_options_empty_string() {
    assert!(parse_options("", MAX).is_err());
  }

  #[test]
  fn parse_options_invalid() {
    assert!(parse_options("invalid-xyz-123", MAX).is_err());
  }

  #[test]
  fn parse_options_trim() {
    let opts = parse_options("tr-s200x200", MAX).unwrap();
    assert_eq!(opts.len(), 2); // trim + scale
  }
}
