use anyhow::Result;
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
  pub original_base_url: Option<String>,
}

#[derive(Deserialize)]
pub struct StorageConfigLocal {
  pub path: String,
}

pub fn parse(config_path: &str) -> Result<Config> {
  // Load config
  let toml_str = fs::read_to_string(config_path).expect("failed to read config file");
  let cfg: Config = toml::from_str(&toml_str).expect("failed to deserialize config");

  Ok(cfg)
}
