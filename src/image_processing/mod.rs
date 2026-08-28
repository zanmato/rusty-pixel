use axum::body::Bytes;
use libvips::VipsImage;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub mod pipeline;

#[derive(Deserialize, ToSchema)]
#[allow(unused)]
pub struct ProcessImageForm {
  details: ImageProcessingRequest,
  #[schema(format = Binary, content_media_type = "application/octet-stream")]
  image: String,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ImageProcessingRequest {
  pub id: String,
  pub path: String,
  pub min_size: Option<i32>,
  pub save_original: bool,
  pub portrait_environment_image: Option<EnvironmentImage>,
  pub landscape_environment_image: Option<EnvironmentImage>,
  pub configurations: Vec<ImageConfiguration>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ImageConfiguration {
  pub id: String,
  pub path: String,
  pub aspect: f64,
  pub margin_percent: i32,
  pub size: i32,
  pub quality: i32,
  pub conditions: ImageConditions,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ImageConditions {
  pub transparent: bool,
  pub trim: bool,
  pub black_and_white: bool,
  pub use_environment_image: bool,
  pub allow_vector: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, ToSchema)]
pub struct EnvironmentImage {
  pub path: String,
  pub width: i32,
  pub height: i32,
  pub x: i32,
  pub y: i32,
  pub margin_percent: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone, ToSchema)]
pub struct ProcessedImage {
  pub id: String,
  pub alternative_to: Option<String>,
  pub path: String,
  pub url: String,
  pub mime: String,
  pub hash: String,
  pub size: u64,
  pub width: i32,
  pub height: i32,
}

/// An encoded output waiting to be uploaded.
#[derive(Debug, Clone)]
pub struct UploadImage {
  /// Position in the response, so results keep the order of the request even
  /// though configurations are processed in parallel.
  pub order: usize,
  pub id: String,
  pub alternative_to: Option<String>,
  pub mime: String,
  pub path: String,
  pub data: Bytes,
  pub width: i32,
  pub height: i32,
}

/// A decoded libvips image that may be read from several threads at once.
///
/// `VipsImage` wraps a raw pointer and is therefore neither `Send` nor `Sync`
/// in the binding, but libvips itself is designed for exactly this: images are
/// reference counted GObjects and reading pixels from one image on several
/// threads is supported and is how libvips' own thread pool works. Only
/// reading is exposed here (`Deref` to `&VipsImage`), never mutation.
pub struct SharedImage(VipsImage);

unsafe impl Send for SharedImage {}
unsafe impl Sync for SharedImage {}

impl SharedImage {
  pub fn new(img: VipsImage) -> Self {
    SharedImage(img)
  }
}

impl std::ops::Deref for SharedImage {
  type Target = VipsImage;

  fn deref(&self) -> &VipsImage {
    &self.0
  }
}

pub fn loader_to_mime_ext(loader: &str) -> (&'static str, &'static str) {
  match loader {
    "jpegload_buffer" => ("image/jpeg", "jpg"),
    "jxlload_buffer" => ("image/jxl", "jxl"),
    "magickload_buffer" => ("application/octet-stream", "magick"),
    "pngload_buffer" => ("image/png", "png"),
    "radload_buffer" => ("image/x-radiance", "rad"),
    "svgload_buffer" => ("image/svg+xml", "svg"),
    "tiffload_buffer" => ("image/tiff", "tiff"),
    "webpload_buffer" => ("image/webp", "webp"),
    "pdfload_buffer" => ("application/pdf", "pdf"),
    "jp2kload_buffer" => ("image/jp2", "jp2"),
    "heifload_buffer" => ("image/heif", "heif"),
    "gifload_buffer" => ("image/gif", "gif"),
    _ => ("application/octet-stream", "bin"),
  }
}

pub fn alternative_possible(loader: &str, allow_vector: bool) -> bool {
  match loader {
    "jpegload_buffer" => true,
    "jxlload_buffer" => true,
    "magickload_buffer" => true,
    "pngload_buffer" => true,
    "radload_buffer" => false,
    "svgload_buffer" => !allow_vector,
    "tiffload_buffer" => true,
    "webpload_buffer" => true,
    "pdfload_buffer" => false,
    "jp2kload_buffer" => true,
    "heifload_buffer" => true,
    "gifload_buffer" => true,
    _ => false,
  }
}
