use lazy_static::lazy_static;
use libvips::{ops, VipsImage};
use regex::Regex;

use super::ImageModifier;
use crate::image_modifier::util;

pub struct ScaleModifier {
  aspect: f64,
  margin_percentage: i32,
  size: Option<i32>,
  crop: bool,
}

lazy_static! {
  static ref SCALE_REGEX: Regex = Regex::new(r"^s(\d+)x(\d+)$").unwrap();
  static ref MARGIN_REGEX: Regex = Regex::new(r"^m(\d+)$").unwrap();
}

impl ScaleModifier {
  pub fn new(aspect: f64, margin_percentage: i32, size: Option<i32>, crop: bool) -> ScaleModifier {
    ScaleModifier {
      aspect,
      margin_percentage,
      size,
      crop,
    }
  }

  pub fn evaluate(opt: &str, opts: &[&str]) -> Option<Box<dyn ImageModifier>> {
    if let Some(captures) = SCALE_REGEX.captures(opt) {
      if let (Ok(width), Ok(height)) = (captures[1].parse(), captures[2].parse()) {
        let mut sopt = ScaleModifier {
          aspect: util::aspect(width, height),
          margin_percentage: 0,
          size: None,
          crop: true,
        };

        // Check if there's a margin option
        for o in opts {
          if let Some(margin_captures) = MARGIN_REGEX.captures(o) {
            if let Ok(margin) = margin_captures[1].parse() {
              sopt.margin_percentage = margin;
              break;
            }
          }
        }

        return Some(Box::new(sopt));
      }
    }

    None
  }
}

impl ImageModifier for ScaleModifier {
  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    let source_width = img.get_width() as f64;
    let source_height = img.get_height() as f64;

    let aspect = if self.aspect == 0.0 {
      // Recalculate aspect ratio
      util::aspect(source_width as i32, source_height as i32)
    } else {
      self.aspect
    };

    let (margin, area_height, area_width, new_width, new_height): (f64, i32, i32, i32, i32);

    if source_width > source_height {
      let base = if self.size.is_some() {
        self.size.unwrap()
      } else {
        source_width as i32
      };

      margin = self.margin_percentage as f64 * 0.01 * (base as f64 / aspect);
      new_height = (base as f64 / aspect - margin).floor() as i32;
      new_width = (base as f64 - margin) as i32;

      area_height = (base as f64 / aspect).floor() as i32;
      area_width = base;
    } else {
      let base = if self.size.is_some() {
        self.size.unwrap()
      } else {
        source_height as i32
      };

      margin = self.margin_percentage as f64 * 0.01 * (base as f64 / aspect);
      new_width = (base as f64 / aspect - margin).floor() as i32;
      new_height = (base as f64 - margin) as i32;

      area_width = (base as f64 / aspect).floor() as i32;
      area_height = base;
    }

    let thumb = ops::thumbnail_image_with_opts(
      img,
      new_width,
      &ops::ThumbnailImageOptions {
        height: new_height,
        size: ops::Size::Both,
        crop: match self.crop {
          true => ops::Interesting::Centre,
          false => ops::Interesting::None,
        },
        export_profile: "sRGB".to_owned(),
        import_profile: "sRGB".to_owned(),
        ..ops::ThumbnailImageOptions::default()
      },
    )?;

    Ok(Some(ops::gravity_with_opts(
      &thumb,
      ops::CompassDirection::Centre,
      area_width,
      area_height,
      &ops::GravityOptions {
        extend: ops::Extend::White,
        background: vec![255.0, 255.0, 255.0],
      },
    )?))
  }
}
