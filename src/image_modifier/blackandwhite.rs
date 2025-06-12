use libvips::{ops, VipsImage};

use super::ImageModifier;

pub struct BlackAndWhiteModifier;

impl BlackAndWhiteModifier {
  pub fn evaluate(opt: &str, _opts: &[&str]) -> Option<Box<dyn ImageModifier>> {
    if opt == "bw" {
      Some(Box::new(BlackAndWhiteModifier))
    } else {
      None
    }
  }
}

impl ImageModifier for BlackAndWhiteModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    Ok(Some(ops::colourspace(img, ops::Interpretation::BW)?))
  }
}
