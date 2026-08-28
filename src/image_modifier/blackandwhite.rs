use libvips::{VipsImage, ops};

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
    // Converting to B_W keeps any embedded RGB profile, which no longer matches
    // the single band image and makes later thumbnails warn "profile
    // incompatible with image". Going back to sRGB keeps the grey result while
    // the pixels and the profile agree again.
    let grey = ops::colourspace(img, ops::Interpretation::BW)?;
    Ok(Some(ops::colourspace(&grey, ops::Interpretation::Srgb)?))
  }
}
