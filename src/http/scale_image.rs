use axum::{
  extract::{Path, State},
  http::{header, StatusCode},
  response::IntoResponse,
};
use libvips::{ops, VipsImage};
use std::mem;
use tracing::error;

use crate::http::AppState;
use crate::image_modifier;

use crate::http::error::AppError;

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

    let mut output_image = VipsImage::new_from_buffer(&data, "").unwrap();
    for opt in modifiers {
      let modifier_res = opt.apply(&output_image);
      if let Err(e) = modifier_res {
        mem::drop(data);
        let _ = send.send(Err(e.to_string()));
        return;
      }

      if let Some(m) = modifier_res.unwrap() {
        output_image = m;
      }
    }

    let res = ops::jpegsave_buffer_with_opts(
      &output_image,
      &ops::JpegsaveBufferOptions {
        q: 80,
        background: vec![255.0, 255.0, 255.0],
        profile: "sRGB".to_owned(),
        ..ops::JpegsaveBufferOptions::default()
      },
    );

    if let Err(e) = res {
      mem::drop(data);
      let _ = send.send(Err(e.to_string()));
      return;
    }

    let _ = send.send(Ok(res.unwrap()));

    // The vips image will hold a reference to the buffer, so we need to drop it
    mem::drop(data);
  });

  let recv_res = recv.await.map_err(|e| {
    error!("failed to receive: {}", e);
    e
  });

  let final_image = recv_res.unwrap();

  if final_image.is_err() {
    error!(
      "failed to transform image: {} {}",
      final_image.unwrap_err(),
      state.vips_app.error_buffer().unwrap_or("")
    );
    return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
  }
  let headers = [(header::CONTENT_TYPE, "image/jpeg")];

  (StatusCode::OK, headers, final_image.unwrap()).into_response()
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
