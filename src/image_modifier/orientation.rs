use libvips::{VipsImage, ops};

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
      // Use the exact 90-degree rotate (`vips_rot`) rather than the arbitrary-angle
      // `rotate_with_opts`: the latter's generated binding always passes the static
      // `interpolate` singleton, whose refcount libvips corrupts over repeated calls,
      // causing a `g_object_unref` crash under load. `rot` needs no interpolation or
      // background and swaps the dimensions exactly.
      return Ok(Some(ops::rot(img, ops::Angle::D90)?));
    }

    Ok(None)
  }
}
