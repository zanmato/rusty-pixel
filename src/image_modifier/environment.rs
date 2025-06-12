use std::sync::Arc;

use libvips::{ops, VipsImage};

use super::ImageModifier;

#[derive(Clone)]
pub struct EnvironmentOptions {
  pub width: i32,
  pub height: i32,
  pub x: i32,
  pub y: i32,
  pub margin_percent: i32,
}

pub struct EnvironmentModifier {
  env_image: Arc<Vec<u8>>,
  opts: EnvironmentOptions,
}

impl EnvironmentModifier {
  pub fn new(env_image: Arc<Vec<u8>>, opts: EnvironmentOptions) -> EnvironmentModifier {
    EnvironmentModifier { env_image, opts }
  }
}

impl ImageModifier for EnvironmentModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    let env_image = VipsImage::new_from_buffer(&self.env_image, "").unwrap();

    // scale input image
    let scaled = ops::thumbnail_image_with_opts(
      img,
      self.opts.width,
      &ops::ThumbnailImageOptions {
        height: self.opts.height,
        size: ops::Size::Both,
        crop: ops::Interesting::Centre,
        export_profile: "sRGB".to_owned(),
        import_profile: "sRGB".to_owned(),
        ..ops::ThumbnailImageOptions::default()
      },
    )?;

    // composite with env image
    Ok(Some(ops::composite_2_with_opts(
      &env_image,
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
