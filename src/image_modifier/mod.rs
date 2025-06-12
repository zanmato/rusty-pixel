use libvips::VipsImage;

mod util;

pub mod blackandwhite;
pub mod environment;
pub mod orientation;
pub mod resize;
pub mod scale;
pub mod trim;

pub trait ImageModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>>;
}

pub type ImageModifierEvaluator = fn(&str, &[&str]) -> Option<Box<dyn ImageModifier>>;
