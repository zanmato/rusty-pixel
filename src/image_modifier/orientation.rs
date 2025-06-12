use libvips::{ops, VipsImage};

use super::ImageModifier;

pub struct OrientationModifier {
  pub portrait: bool,
}

impl OrientationModifier {
  pub fn evaluate(opt: &str, _opts: &[&str]) -> Option<Box<dyn ImageModifier>> {
    if opt == "oportrait" || opt == "olandscape" {
      return Some(Box::new(OrientationModifier {
        portrait: opt == "oportrait",
      }));
    }

    None
  }
}

impl ImageModifier for OrientationModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    if (self.portrait && img.get_width() > img.get_height())
      || (!self.portrait && img.get_height() > img.get_width())
    {
      return Ok(Some(ops::rotate_with_opts(
        img,
        90.0,
        &ops::RotateOptions {
          background: vec![255.0, 255.0, 255.0],
          ..ops::RotateOptions::default()
        },
      )?));
    }

    Ok(None)
  }
}
