use std::sync::LazyLock;

use libvips::{VipsImage, ops};
use regex::Regex;

use super::ImageModifier;
use crate::image_modifier::{border, util};

pub struct ScaleModifier {
  aspect: f64,
  margin_percentage: i32,
  size: Option<i32>,
  crop: bool,
  min_side: Option<i32>,
  min_side_max_scale: Option<f64>,
}

/// The pixel geometry a scale produces for a given source size: the image is
/// scaled to fit `width` x `height` and then centred on a canvas of
/// `area_width` x `area_height`.
///
/// `padded` is set when the canvas was grown to honour a minimum side. That
/// extra space is filled with the colour of the image's corners instead of white.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalePlan {
  pub width: i32,
  pub height: i32,
  pub area_width: i32,
  pub area_height: i32,
  pub crop: bool,
  pub padded: bool,
}

impl ScalePlan {
  fn short_side(&self) -> i32 {
    self.area_width.min(self.area_height)
  }

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
      min_side: None,
      min_side_max_scale: None,
    }
  }

  /// Guarantee that neither side of the output is shorter than `min_side`.
  /// The image keeps its scale and the canvas grows around it, so a wide
  /// 1024 x 400 result with a minimum of 500 becomes 1024 x 500.
  ///
  /// With `max_scale` an image that is close to the minimum is scaled up to
  /// reach it instead of being padded: the longest side may grow to at most
  /// `size * max_scale`. 1024 x 467 with a minimum of 500 and a factor of 1.15
  /// becomes 1096 x 500, while 1024 x 409 would need 1.22 and is padded.
  pub fn with_min_side(mut self, min_side: Option<i32>, max_scale: Option<f64>) -> ScaleModifier {
    self.min_side = min_side.filter(|m| *m > 0);
    self.min_side_max_scale = max_scale.filter(|s| *s > 1.0);
    self
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
        min_side: None,
        min_side_max_scale: None,
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

    let landscape = source_width > source_height;
    let base = self.size.unwrap_or(if landscape {
      source_width
    } else {
      source_height
    });
    let min_side = self.min_side.unwrap_or(0);

    let mut plan = self.geometry(base, aspect, landscape);

    // A canvas that is only a little too short is scaled up past `size`
    // instead of padded, as long as that stays within the allowed factor
    if plan.short_side() < min_side
      && let Some(max_scale) = self.min_side_max_scale
    {
      let limit = (base as f64 * max_scale).floor() as i32;
      let mut scaled = (min_side as f64 * aspect).ceil() as i32;

      // The short side is floored, so the first candidate can be one short
      while scaled <= limit && self.geometry(scaled, aspect, landscape).short_side() < min_side {
        scaled += 1;
      }

      if scaled <= limit {
        plan = self.geometry(scaled, aspect, landscape);
      }
    }

    plan.padded = plan.short_side() < min_side;
    plan.area_width = plan.area_width.max(min_side);
    plan.area_height = plan.area_height.max(min_side);
    plan
  }

  /// The geometry for a canvas whose longest side is `base`.
  fn geometry(&self, base: i32, aspect: f64, landscape: bool) -> ScalePlan {
    let short = base as f64 / aspect;
    let margin = self.margin_percentage as f64 * 0.01 * short;

    let (long, short, area_short) = (
      (base as f64 - margin) as i32,
      (short - margin).floor() as i32,
      short.floor() as i32,
    );

    let (width, height, area_width, area_height) = if landscape {
      (long, short, base, area_short)
    } else {
      (short, long, area_short, base)
    };

    ScalePlan {
      width: width.max(1),
      height: height.max(1),
      area_width: area_width.max(1),
      area_height: area_height.max(1),
      crop: self.crop,
      padded: false,
    }
  }

  /// Centre an already scaled image on the plan's canvas.
  ///
  /// The canvas is white, or transparent when the image has an alpha band. A
  /// canvas that was grown for a minimum side takes the dominant colour of the
  /// image's corners so the padding blends in with the image.
  pub fn frame(thumb: &VipsImage, plan: &ScalePlan) -> Result<VipsImage, libvips::error::Error> {
    if plan.padded {
      // Sampling reads the border before the canvas reads the whole image, so
      // decode once into memory instead of running the pipeline twice
      let thumb = VipsImage::image_copy_memory(ops::copy(thumb)?)?;
      let background = border::dominant_colour(&thumb)?.unwrap_or_else(|| border::blank(&thumb));

      return Self::embed(&thumb, plan, background);
    }

    Self::embed(thumb, plan, border::blank(thumb))
  }

  fn embed(
    thumb: &VipsImage,
    plan: &ScalePlan,
    background: Vec<f64>,
  ) -> Result<VipsImage, libvips::error::Error> {
    ops::gravity_with_opts(
      thumb,
      ops::CompassDirection::Centre,
      plan.area_width,
      plan.area_height,
      &ops::GravityOptions {
        extend: ops::Extend::Background,
        background,
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
  fn plan_min_side_pads_the_short_side() {
    // The featured configuration: source aspect, longest side 1024
    let scale = ScaleModifier::new(0.0, 0, Some(1024), true).with_min_side(Some(500), None);

    let wide = scale.plan(2000, 800);
    assert_eq!((wide.width, wide.height), (1024, 409));
    assert_eq!((wide.area_width, wide.area_height), (1024, 500));
    assert!(wide.padded);

    let tall = scale.plan(800, 2000);
    assert_eq!((tall.width, tall.height), (409, 1024));
    assert_eq!((tall.area_width, tall.area_height), (500, 1024));
    assert!(tall.padded);
  }

  #[test]
  fn plan_min_side_leaves_large_enough_images_alone() {
    let with = ScaleModifier::new(0.0, 0, Some(1024), true).with_min_side(Some(500), None);
    let without = ScaleModifier::new(0.0, 0, Some(1024), true);

    for (w, h) in [(2000, 1000), (1000, 1000), (900, 1600), (1024, 500)] {
      let plan = with.plan(w, h);
      assert!(!plan.padded, "{w}x{h} should not be padded");
      assert_eq!(plan, without.plan(w, h));
    }
  }

  #[test]
  fn plan_min_side_never_goes_below_minimum() {
    let scale = ScaleModifier::new(0.0, 0, Some(1024), true).with_min_side(Some(500), None);

    for (w, h) in [
      (5000, 10),
      (10, 5000),
      (1, 1),
      (255, 102),
      (77, 200),
      (1000, 495),
    ] {
      let plan = scale.plan(w, h);
      assert!(
        plan.area_width >= 500 && plan.area_height >= 500,
        "{w}x{h} gave {plan:?}"
      );
      assert!(plan.area_width.max(plan.area_height) == 1024);
    }
  }

  #[test]
  fn plan_min_side_scales_up_when_close() {
    let scale = ScaleModifier::new(0.0, 0, Some(1024), true).with_min_side(Some(500), Some(1.15));

    // 467 high at 1024, 7 percent short: scaled past 1024, no padding
    let close = scale.plan(2004, 915);
    assert_eq!((close.area_width, close.area_height), (1096, 500));
    assert_eq!((close.width, close.height), (1096, 500));
    assert!(!close.padded);

    let tall = scale.plan(915, 2004);
    assert_eq!((tall.area_width, tall.area_height), (500, 1096));
    assert!(!tall.padded);

    // 409 high at 1024 would need 22 percent: padded at the normal size
    let far = scale.plan(2000, 800);
    assert_eq!((far.width, far.height), (1024, 409));
    assert_eq!((far.area_width, far.area_height), (1024, 500));
    assert!(far.padded);

    // A strip is never blown up
    let strip = scale.plan(1024, 100);
    assert_eq!((strip.width, strip.height), (1024, 100));
    assert_eq!((strip.area_width, strip.area_height), (1024, 500));
    assert!(strip.padded);
  }

  #[test]
  fn plan_min_side_scale_stays_within_the_factor() {
    let scale = ScaleModifier::new(0.0, 0, Some(1024), true).with_min_side(Some(500), Some(1.15));

    for w in 1000..2600 {
      let plan = scale.plan(w, 1000);
      let (long, short) = (
        plan.area_width.max(plan.area_height),
        plan.area_width.min(plan.area_height),
      );
      assert!(short >= 500, "{w}x1000 gave {plan:?}");
      assert!(long <= 1177, "{w}x1000 gave {plan:?}");
      if plan.padded {
        assert_eq!(long, 1024, "{w}x1000 gave {plan:?}");
      } else if long > 1024 {
        assert_eq!(short, 500, "{w}x1000 gave {plan:?}");
      }
    }
  }

  #[test]
  fn plan_min_side_scale_is_ignored_without_min_side() {
    let with = ScaleModifier::new(0.0, 0, Some(1024), true).with_min_side(None, Some(1.15));
    let without = ScaleModifier::new(0.0, 0, Some(1024), true);
    assert_eq!(with.plan(2004, 915), without.plan(2004, 915));
  }

  #[test]
  fn plan_min_side_of_zero_is_ignored() {
    let scale = ScaleModifier::new(0.0, 0, Some(1024), true).with_min_side(Some(0), None);
    assert!(!scale.plan(2000, 800).padded);
  }

  #[test]
  fn plan_without_size_uses_source() {
    let plan = ScaleModifier::new(0.0, 0, None, true).plan(800, 600);
    assert_eq!(plan.area_width, 800);
    assert_eq!(plan.area_height, 600);
  }
}
