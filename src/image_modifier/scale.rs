use std::sync::LazyLock;

use libvips::{VipsImage, ops};
use regex::Regex;

use super::ImageModifier;
use crate::image_modifier::util;

pub struct ScaleModifier {
  aspect: f64,
  margin_percentage: i32,
  size: Option<i32>,
  crop: bool,
}

/// The pixel geometry a scale produces for a given source size: the image is
/// scaled to fit `width` x `height` and then centred on a canvas of
/// `area_width` x `area_height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalePlan {
  pub width: i32,
  pub height: i32,
  pub area_width: i32,
  pub area_height: i32,
  pub crop: bool,
}

impl ScalePlan {
  pub fn interesting(&self) -> ops::Interesting {
    if self.crop {
      ops::Interesting::Centre
    } else {
      ops::Interesting::None
    }
  }
}

static SCALE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^s(\d+)x(\d+)$").unwrap());
static MARGIN_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^m(\d+)$").unwrap());

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
    if let Some(captures) = SCALE_REGEX.captures(opt)
      && let (Ok(width), Ok(height)) = (captures[1].parse(), captures[2].parse())
    {
      let mut sopt = ScaleModifier {
        aspect: util::aspect(width, height),
        margin_percentage: 0,
        size: None,
        crop: true,
      };

      // Check if there's a margin option
      for o in opts {
        if let Some(margin_captures) = MARGIN_REGEX.captures(o)
          && let Ok(margin) = margin_captures[1].parse()
        {
          sopt.margin_percentage = margin;
          break;
        }
      }

      return Some(Box::new(sopt));
    }

    None
  }

  /// Work out the target geometry for a source of the given size without
  /// touching any pixels. This lets callers decode straight to the target size
  /// with `thumbnail_buffer` instead of decoding the full image first.
  pub fn plan(&self, source_width: i32, source_height: i32) -> ScalePlan {
    let aspect = if self.aspect == 0.0 {
      // Recalculate aspect ratio
      util::aspect(source_width, source_height)
    } else {
      self.aspect
    };

    let (margin, area_height, area_width, new_width, new_height): (f64, i32, i32, i32, i32);

    if source_width > source_height {
      let base = self.size.unwrap_or(source_width);

      margin = self.margin_percentage as f64 * 0.01 * (base as f64 / aspect);
      new_height = (base as f64 / aspect - margin).floor() as i32;
      new_width = (base as f64 - margin) as i32;

      area_height = (base as f64 / aspect).floor() as i32;
      area_width = base;
    } else {
      let base = self.size.unwrap_or(source_height);

      margin = self.margin_percentage as f64 * 0.01 * (base as f64 / aspect);
      new_width = (base as f64 / aspect - margin).floor() as i32;
      new_height = (base as f64 - margin) as i32;

      area_width = (base as f64 / aspect).floor() as i32;
      area_height = base;
    }

    ScalePlan {
      width: new_width.max(1),
      height: new_height.max(1),
      area_width: area_width.max(1),
      area_height: area_height.max(1),
      crop: self.crop,
    }
  }

  /// Centre an already scaled image on the plan's canvas.
  pub fn frame(thumb: &VipsImage, plan: &ScalePlan) -> Result<VipsImage, libvips::error::Error> {
    ops::gravity_with_opts(
      thumb,
      ops::CompassDirection::Centre,
      plan.area_width,
      plan.area_height,
      &ops::GravityOptions {
        extend: ops::Extend::White,
        background: vec![255.0, 255.0, 255.0],
      },
    )
  }
}

impl ImageModifier for ScaleModifier {
  fn max_output_dimension(&self) -> Option<i32> {
    self.size
  }

  fn apply(&self, img: &VipsImage) -> Result<Option<VipsImage>, Box<dyn std::error::Error>> {
    let plan = self.plan(img.get_width(), img.get_height());

    let thumb = ops::thumbnail_image_with_opts(
      img,
      plan.width,
      &ops::ThumbnailImageOptions {
        height: plan.height,
        size: ops::Size::Both,
        crop: plan.interesting(),
        output_profile: Some("sRGB".to_owned()),
        ..ops::ThumbnailImageOptions::default()
      },
    )?;

    Ok(Some(Self::frame(&thumb, &plan)?))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn plan_landscape_with_margin() {
    let plan = ScaleModifier::new(1.33, 10, Some(1024), true).plan(4000, 3000);
    assert_eq!(plan.area_width, 1024);
    assert_eq!(plan.area_height, 769);
    assert_eq!(plan.width, 947);
    assert_eq!(plan.height, 692);
    assert!(plan.crop);
  }

  #[test]
  fn plan_portrait_uses_height_as_base() {
    let plan = ScaleModifier::new(1.33, 0, Some(1024), false).plan(3000, 4000);
    assert_eq!(plan.area_height, 1024);
    assert_eq!(plan.area_width, 769);
    assert_eq!(plan.height, 1024);
    assert_eq!(plan.width, 769);
  }

  #[test]
  fn plan_without_size_uses_source() {
    let plan = ScaleModifier::new(0.0, 0, None, true).plan(800, 600);
    assert_eq!(plan.area_width, 800);
    assert_eq!(plan.area_height, 600);
  }
}
