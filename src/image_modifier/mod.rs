use libvips::VipsImage;

mod util;

pub mod blackandwhite;
pub mod environment;
pub mod orientation;
pub mod resize;
pub mod scale;
pub mod trim;

pub trait ImageModifier: Send {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>>;

  /// The largest side this modifier can produce regardless of input, if it
  /// sets an absolute size. Used to reject oversized requests up front.
  fn max_output_dimension(&self) -> Option<i32> {
    None
  }
}

pub type ImageModifierEvaluator = fn(&str, &[&str]) -> Option<Box<dyn ImageModifier>>;
