use std::sync::Arc;

use libvips::{VipsImage, ops};

use super::ImageModifier;
use crate::image_processing::SharedImage;

#[derive(Clone)]
pub struct EnvironmentOptions {
  pub width: i32,
  pub height: i32,
  pub x: i32,
  pub y: i32,
  pub margin_percent: i32,
}

/// Composites the image onto a decoded environment image. The environment
/// image is decoded once per request and shared between configurations.
pub struct EnvironmentModifier {
  env_image: Arc<SharedImage>,
  opts: EnvironmentOptions,
}

impl EnvironmentModifier {
  pub fn new(env_image: Arc<SharedImage>, opts: EnvironmentOptions) -> EnvironmentModifier {
    EnvironmentModifier { env_image, opts }
  }
}

impl ImageModifier for EnvironmentModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    // scale input image
    let scaled = ops::thumbnail_image_with_opts(
      img,
      self.opts.width,
      &ops::ThumbnailImageOptions {
        height: self.opts.height,
        size: ops::Size::Both,
        crop: ops::Interesting::Centre,
        output_profile: Some("sRGB".to_owned()),
        input_profile: Some("sRGB".to_owned()),
        ..ops::ThumbnailImageOptions::default()
      },
    )?;

    // composite with env image
    Ok(Some(ops::composite2_with_opts(
      &self.env_image,
      &scaled,
      ops::BlendMode::DestOver,
      &ops::Composite2Options {
        x: self.opts.x,
        y: self.opts.y,
        ..ops::Composite2Options::default()
      },
    )?))
  }
}
