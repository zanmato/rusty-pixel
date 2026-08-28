//! The synchronous, CPU bound part of the process image endpoint.
//!
//! Everything in here runs on the rayon pool. Configurations are independent
//! of each other so they are processed in parallel, each one decoding the
//! source straight to its target size with `thumbnail_buffer`. That lets
//! libvips shrink on load: JPEGs are decoded at 1/2, 1/4 or 1/8 scale through
//! libjpeg's DCT scaling, WebP and HEIF use their reduced size decoders, and
//! SVG or PDF sources are rendered directly at the target resolution.
//!
//! Configurations that trim need the full frame before scaling, so those share
//! one decoded base image that is materialised once at twice the largest
//! requested size. Trimmed content is rarely more than half the frame, so this
//! keeps enough resolution for the final scale while still shrinking on load.

use std::sync::{
  Arc, OnceLock,
  atomic::{AtomicBool, Ordering},
};

use axum::body::Bytes;
use libvips::{VipsApp, VipsImage, ops};
use rayon::prelude::*;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::http::metrics::{self, Timer};
use crate::image_modifier::{
  self, ImageModifier,
  environment::{EnvironmentModifier, EnvironmentOptions},
  scale::ScaleModifier,
};
use crate::image_processing::{
  self, ImageConfiguration, ImageProcessingRequest, SharedImage, UploadImage,
};

const WHITE: [f64; 3] = [255.0, 255.0, 255.0];

/// Header information about the uploaded source, read without decoding pixels.
#[derive(Debug, Clone)]
pub struct SourceInfo {
  /// Width after EXIF orientation is applied, which is what thumbnails see.
  pub width: i32,
  pub height: i32,
  pub loader: String,
}

impl SourceInfo {
  pub fn is_portrait(&self) -> bool {
    self.width < self.height
  }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PipelineError {
  #[error("invalid image: {0}")]
  InvalidImage(String),
  #[error("request cancelled")]
  Cancelled,
  #[error("{0}")]
  Failed(String),
}

/// Read the source header. Opening a buffer in libvips is lazy so this only
/// parses the header, no pixels are decoded.
pub fn source_info(data: &[u8], vips: &VipsApp) -> Result<SourceInfo, PipelineError> {
  let image = VipsImage::new_from_buffer(data, "")
    .map_err(|e| PipelineError::InvalidImage(describe(vips, "failed to load image", e)))?;

  let loader = image
    .get_as_string("vips-loader")
    .map_err(|e| PipelineError::InvalidImage(describe(vips, "failed to read loader", e)))?;

  let (mut width, mut height) = (image.get_width(), image.get_height());

  // Orientations 5 to 8 rotate by 90 degrees, swapping the axes once
  // thumbnail applies the tag.
  if image.get_orientation() >= 5 {
    std::mem::swap(&mut width, &mut height);
  }

  Ok(SourceInfo {
    width,
    height,
    loader,
  })
}

/// Decoded environment image plus its placement, shared by all configurations.
pub struct Environment {
  pub image: Arc<SharedImage>,
  pub opts: EnvironmentOptions,
}

impl Environment {
  pub fn decode(
    data: &[u8],
    opts: EnvironmentOptions,
    vips: &VipsApp,
  ) -> Result<Self, PipelineError> {
    let image = VipsImage::new_from_buffer(data, "")
      .and_then(VipsImage::image_copy_memory)
      .map_err(|e| PipelineError::Failed(describe(vips, "failed to load environment image", e)))?;

    Ok(Environment {
      image: Arc::new(SharedImage::new(image)),
      opts,
    })
  }
}

pub struct Pipeline {
  pub data: Bytes,
  pub source: SourceInfo,
  pub request: ImageProcessingRequest,
  pub environment: Option<Environment>,
  pub vips: Arc<VipsApp>,
  /// Set by the HTTP layer when the client goes away or the request times
  /// out. Checked between expensive steps so abandoned work stops early.
  pub cancel: Arc<AtomicBool>,
}

impl Pipeline {
  /// Process every configuration, sending each encoded output on `tx` as soon
  /// as it is ready so uploads overlap with encoding.
  pub fn run(self, tx: mpsc::Sender<UploadImage>) -> Result<(), PipelineError> {
    let result = self.run_configurations(tx);

    // Several configurations can notice the cancellation at once, so count it
    // here where it is seen exactly once per request
    if matches!(result, Err(PipelineError::Cancelled)) {
      ::metrics::counter!(metrics::CANCELLED).increment(1);
    }

    result
  }

  fn run_configurations(&self, tx: mpsc::Sender<UploadImage>) -> Result<(), PipelineError> {
    let base = OnceLock::new();
    let base_size = self.base_size();

    let result = self
      .request
      .configurations
      .par_iter()
      .enumerate()
      .try_for_each(|(index, config)| {
        self.check_cancelled()?;

        let base = if config.conditions.trim && !self.passthrough(config) {
          Some(
            base
              .get_or_init(|| self.decode_base(base_size))
              .as_ref()
              .map_err(Clone::clone)?,
          )
        } else {
          None
        };

        for image in self.process(index, config, base)? {
          tx.blocking_send(image)
            .map_err(|_| PipelineError::Cancelled)?;
        }

        Ok(())
      });

    result?;

    if self.request.save_original {
      let meta = image_processing::loader_to_mime_ext(&self.source.loader);
      tx.blocking_send(UploadImage {
        order: self.request.configurations.len() * 2,
        path: format!("{}.{}", self.request.path, meta.1),
        id: self.request.id.clone(),
        data: self.data.clone(),
        mime: meta.0.to_owned(),
        alternative_to: None,
        width: self.source.width,
        height: self.source.height,
      })
      .map_err(|_| PipelineError::Cancelled)?;
    }

    Ok(())
  }

  fn check_cancelled(&self) -> Result<(), PipelineError> {
    if self.cancel.load(Ordering::Relaxed) {
      return Err(PipelineError::Cancelled);
    }
    Ok(())
  }

  fn passthrough(&self, config: &ImageConfiguration) -> bool {
    config.conditions.allow_vector && self.source.loader == "svgload_buffer"
  }

  /// Longest side of the shared base image used by trimming configurations.
  fn base_size(&self) -> i32 {
    let largest = self
      .request
      .configurations
      .iter()
      .filter(|c| c.conditions.trim)
      .map(|c| c.size)
      .max()
      .unwrap_or(0);

    (largest * 2).clamp(1, self.source.width.max(self.source.height).max(1))
  }

  fn decode_base(&self, size: i32) -> Result<SharedImage, PipelineError> {
    let timer = Timer::start();
    let image = ops::thumbnail_buffer_with_opts(
      &self.data,
      size,
      &ops::ThumbnailBufferOptions {
        height: size,
        size: ops::Size::Down,
        crop: ops::Interesting::None,
        output_profile: Some("sRGB".to_owned()),
        ..ops::ThumbnailBufferOptions::default()
      },
    )
    .and_then(VipsImage::image_copy_memory)
    .map_err(|e| PipelineError::Failed(describe(&self.vips, "failed to decode base image", e)))?;
    timer.observe(metrics::DECODE_DURATION);

    debug!(size, "decoded shared base image");

    Ok(SharedImage::new(image))
  }

  fn process(
    &self,
    index: usize,
    config: &ImageConfiguration,
    base: Option<&SharedImage>,
  ) -> Result<Vec<UploadImage>, PipelineError> {
    let order = index * 2;

    // Pass the vector as is
    if self.passthrough(config) {
      return Ok(vec![UploadImage {
        order,
        path: format!("{}.svg", config.path),
        mime: "image/svg+xml".to_string(),
        id: config.id.clone(),
        data: self.data.clone(),
        alternative_to: None,
        width: self.source.width,
        height: self.source.height,
      }]);
    }

    let decode = Timer::start();
    let mut image = match base {
      Some(base) => self.scale_from_base(config, base)?,
      None => self.scale_from_source(config)?,
    };
    decode.observe(metrics::DECODE_DURATION);

    if config.conditions.use_environment_image
      && let Some(env) = &self.environment
    {
      let modifier = EnvironmentModifier::new(env.image.clone(), env.opts.clone());
      image = self.apply(&modifier, image)?;
    }

    self.check_cancelled()?;

    let width = image.get_width();
    let height = image.get_height();

    let (ext, mime, data) = if config.conditions.transparent {
      ("png", "image/png", self.encode_png(&image)?)
    } else {
      (
        "jpg",
        "image/jpeg",
        self.encode_jpeg(&image, config.quality)?,
      )
    };

    let mut outputs = vec![UploadImage {
      order,
      path: format!("{}.{}", config.path, ext),
      mime: mime.to_owned(),
      id: config.id.clone(),
      data,
      alternative_to: None,
      width,
      height,
    }];

    if image_processing::alternative_possible(&self.source.loader, config.conditions.allow_vector) {
      self.check_cancelled()?;

      outputs.push(UploadImage {
        order: order + 1,
        path: format!("{}.webp", config.path),
        id: Uuid::new_v4().into(),
        data: self.encode_webp(&image, config.quality)?,
        mime: "image/webp".to_owned(),
        alternative_to: Some(config.id.clone()),
        width,
        height,
      });
    }

    Ok(outputs)
  }

  /// Fast path: decode straight to the target size, then greyscale and frame.
  fn scale_from_source(&self, config: &ImageConfiguration) -> Result<VipsImage, PipelineError> {
    let scale = ScaleModifier::new(
      config.aspect,
      config.margin_percent,
      Some(config.size),
      true,
    );
    let plan = scale.plan(self.source.width, self.source.height);

    let thumb = ops::thumbnail_buffer_with_opts(
      &self.data,
      plan.width,
      &ops::ThumbnailBufferOptions {
        height: plan.height,
        size: ops::Size::Both,
        crop: plan.interesting(),
        output_profile: Some("sRGB".to_owned()),
        ..ops::ThumbnailBufferOptions::default()
      },
    )
    .map_err(|e| PipelineError::Failed(describe(&self.vips, "failed to decode image", e)))?;

    let thumb = if config.conditions.black_and_white {
      self.apply(&image_modifier::blackandwhite::BlackAndWhiteModifier, thumb)?
    } else {
      thumb
    };

    ScaleModifier::frame(&thumb, &plan)
      .map_err(|e| PipelineError::Failed(describe(&self.vips, "failed to frame image", e)))
  }

  /// Trim path: greyscale and trim the shared base, then scale without crop
  /// so the trimmed content is never cut.
  fn scale_from_base(
    &self,
    config: &ImageConfiguration,
    base: &SharedImage,
  ) -> Result<VipsImage, PipelineError> {
    let mut image = ops::copy(base)
      .map_err(|e| PipelineError::Failed(describe(&self.vips, "failed to copy base image", e)))?;

    if config.conditions.black_and_white {
      image = self.apply(&image_modifier::blackandwhite::BlackAndWhiteModifier, image)?;
    }

    image = self.apply(
      &image_modifier::trim::TrimModifier::new(WHITE.to_vec()),
      image,
    )?;

    let scale = ScaleModifier::new(
      config.aspect,
      config.margin_percent,
      Some(config.size),
      false,
    );
    self.apply(&scale, image)
  }

  fn apply(
    &self,
    modifier: &dyn ImageModifier,
    image: VipsImage,
  ) -> Result<VipsImage, PipelineError> {
    match modifier.apply(&image) {
      Ok(Some(modified)) => Ok(modified),
      Ok(None) => Ok(image),
      Err(e) => Err(PipelineError::Failed(format!(
        "failed to apply modifier: {} {}",
        e,
        self.take_vips_error()
      ))),
    }
  }

  fn encode_png(&self, image: &VipsImage) -> Result<Bytes, PipelineError> {
    let timer = Timer::start();
    let data = ops::pngsave_buffer_with_opts(
      image,
      &ops::PngsaveBufferOptions {
        keep: ops::ForeignKeep::None,
        ..ops::PngsaveBufferOptions::default()
      },
    )
    .map_err(|e| PipelineError::Failed(describe(&self.vips, "failed to encode png", e)))?;
    self.record_output(timer, "png", data.len());

    Ok(Bytes::from(data))
  }

  fn encode_jpeg(&self, image: &VipsImage, quality: i32) -> Result<Bytes, PipelineError> {
    let timer = Timer::start();
    let data = ops::jpegsave_buffer_with_opts(
      image,
      &ops::JpegsaveBufferOptions {
        q: quality,
        optimize_coding: true,
        background: WHITE.to_vec(),
        keep: ops::ForeignKeep::None,
        ..ops::JpegsaveBufferOptions::default()
      },
    )
    .map_err(|e| PipelineError::Failed(describe(&self.vips, "failed to encode jpeg", e)))?;
    self.record_output(timer, "jpeg", data.len());

    Ok(Bytes::from(data))
  }

  fn encode_webp(&self, image: &VipsImage, quality: i32) -> Result<Bytes, PipelineError> {
    let timer = Timer::start();
    let data = ops::webpsave_buffer_with_opts(
      image,
      &ops::WebpsaveBufferOptions {
        q: quality,
        // libwebp's default. Higher is slower for a few percent smaller files
        effort: 4,
        background: WHITE.to_vec(),
        keep: ops::ForeignKeep::None,
        ..ops::WebpsaveBufferOptions::default()
      },
    )
    .map_err(|e| PipelineError::Failed(describe(&self.vips, "failed to encode webp", e)))?;
    self.record_output(timer, "webp", data.len());

    Ok(Bytes::from(data))
  }

  fn record_output(&self, timer: Timer, format: &'static str, bytes: usize) {
    timer.observe_with(metrics::ENCODE_DURATION, "format", format);
    ::metrics::histogram!(metrics::OUTPUT_BYTES, "format" => format).record(bytes as f64);
  }

  fn take_vips_error(&self) -> String {
    take_vips_error(&self.vips)
  }
}

/// The binding's errors are static names such as `JpegsaveBufferError`. The
/// actual reason lives in libvips' global error buffer, so read and clear it
/// right away, before another thread adds to it.
fn take_vips_error(vips: &VipsApp) -> String {
  let message = vips
    .error_buffer()
    .map(|s| s.trim().to_owned())
    .unwrap_or_default();
  vips.error_clear();

  if message.is_empty() {
    warn!("libvips reported an error without a message");
  }

  message
}

fn describe(vips: &VipsApp, context: &str, err: libvips::error::Error) -> String {
  format!("{}: {} {}", context, err, take_vips_error(vips))
}
