use lazy_static::lazy_static;
use libvips::{ops, VipsImage};
use regex::Regex;

use super::ImageModifier;
use crate::image_modifier::util;

lazy_static! {
  static ref RESIZE_REGEX: Regex = Regex::new(r"^r(w|h)(\d+)$").unwrap();
}

pub struct ResizeModifier {
  height: bool,
  pixels: i32,
}

impl ResizeModifier {
  pub fn evaluate(opt: &str, _opts: &[&str]) -> Option<Box<dyn ImageModifier>> {
    if let Some(matches) = RESIZE_REGEX.captures(opt) {
      let height = matches.get(1).unwrap().as_str() == "h";
      let pixels = matches.get(2).unwrap().as_str().parse::<i32>().ok()?;

      return Some(Box::new(ResizeModifier { height, pixels }));
    }

    None
  }
}

impl ImageModifier for ResizeModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    if self.height {
      return Ok(Some(ops::thumbnail_image_with_opts(
        img,
        (self.pixels as f64 * util::aspect(img.get_width(), img.get_height())) as i32,
        &ops::ThumbnailImageOptions {
          height: self.pixels,
          size: ops::Size::Both,
          crop: ops::Interesting::None,
          export_profile: "sRGB".to_owned(),
          import_profile: "sRGB".to_owned(),
          ..ops::ThumbnailImageOptions::default()
        },
      )?));
    }

    Ok(Some(ops::thumbnail_image_with_opts(
      img,
      self.pixels,
      &ops::ThumbnailImageOptions {
        height: (self.pixels as f64 * util::aspect(img.get_width(), img.get_height())) as i32,
        size: ops::Size::Both,
        crop: ops::Interesting::None,
        export_profile: "sRGB".to_owned(),
        import_profile: "sRGB".to_owned(),
        ..ops::ThumbnailImageOptions::default()
      },
    )?))
  }
}
