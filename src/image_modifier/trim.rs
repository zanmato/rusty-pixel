use libvips::{ops, VipsImage};

use super::ImageModifier;

pub struct TrimModifier {
  background: Vec<f64>,
}

impl TrimModifier {
  pub fn new(background: Vec<f64>) -> TrimModifier {
    TrimModifier { background }
  }

  pub fn evaluate(opt: &str, _opts: &[&str]) -> Option<Box<dyn ImageModifier>> {
    if opt == "tr" {
      Some(Box::new(TrimModifier {
        background: vec![255.0, 255.0, 255.0],
      }))
    } else {
      None
    }
  }
}

impl ImageModifier for TrimModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    let area = ops::find_trim_with_opts(
      img,
      &ops::FindTrimOptions {
        background: self.background.clone(),
        ..ops::FindTrimOptions::default()
      },
    )?;

    Ok(Some(ops::extract_area(
      img, area.0, area.1, area.2, area.3,
    )?))
  }
}
