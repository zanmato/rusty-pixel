use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;

#[derive(Deserialize)]
pub enum StorageType {
  Local,
  S3,
}

#[derive(Deserialize)]
pub struct Config {
  pub app: AppConfig,
  pub storage: StorageConfig,
}

#[derive(Deserialize)]
pub struct AppConfig {
  pub listen: String,
  pub metrics_listen: String,
  pub vips_concurrency: i32,
  pub api_key: String,
  pub max_body_size_mb: usize,
  pub enable_openapi: Option<bool>,
  /// Size of the image processing thread pool. Defaults to the number of CPUs.
  /// Each worker also drives `vips_concurrency` libvips threads, so keep the
  /// product of the two close to the CPU count.
  pub worker_threads: Option<usize>,
  /// How many uploads to storage may be in flight per request. Defaults to 4.
  pub upload_concurrency: Option<usize>,
  /// Largest side any output may have. Requests asking for more are rejected.
  /// Defaults to 4096.
  pub max_output_dimension: Option<i32>,
  /// JPEG quality for the public scale endpoint. Defaults to 80.
  pub scale_quality: Option<i32>,
  /// Cache-Control header sent with scale endpoint responses.
  /// Defaults to "public, max-age=31536000, immutable".
  pub scale_cache_control: Option<String>,
}

#[derive(Deserialize)]
pub struct StorageConfig {
  pub storage_type: StorageType,
  pub s3: Option<StorageConfigS3>,
  pub local: Option<StorageConfigLocal>,
}

#[derive(Deserialize)]
pub struct StorageConfigS3 {
  pub endpoint: String,
  pub bucket: String,
  pub access_key_id: String,
  pub secret_access_key: String,
  pub region: String,
  pub force_path_style: bool,
  pub base_url: String,
}

#[derive(Deserialize)]
pub struct StorageConfigLocal {
  pub path: String,
}

pub fn parse(config_path: &str) -> Result<Config> {
  // Load config
  let toml_str = fs::read_to_string(config_path)
    .with_context(|| format!("failed to read config file: {}", config_path))?;
  let cfg: Config = toml::from_str(&toml_str).context("failed to deserialize config")?;

  Ok(cfg)
}
