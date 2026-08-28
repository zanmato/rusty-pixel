use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;

/// Environment variables with this prefix override values from the TOML file.
/// Nested keys are separated by a double underscore, for example
/// `RP__STORAGE__S3__SECRET_ACCESS_KEY` sets `storage.s3.secret_access_key`.
pub const ENV_PREFIX: &str = "RP__";

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
  #[serde(deserialize_with = "lenient::required")]
  pub vips_concurrency: i32,
  /// libvips operation cache limit. 0 disables the cache, which is the right
  /// choice for a service where every request carries a different image.
  #[serde(default, deserialize_with = "lenient::required")]
  pub vips_cache_max_mem_mb: u64,
  pub api_key: String,
  #[serde(deserialize_with = "lenient::required")]
  pub max_body_size_mb: usize,
  #[serde(default, deserialize_with = "lenient::optional")]
  pub enable_openapi: Option<bool>,
  /// Size of the image processing thread pool. Defaults to the number of CPUs.
  /// Each worker also drives `vips_concurrency` libvips threads, so keep the
  /// product of the two close to the CPU count.
  #[serde(default, deserialize_with = "lenient::optional")]
  pub worker_threads: Option<usize>,
  /// How many uploads to storage may be in flight per request. Defaults to 4.
  #[serde(default, deserialize_with = "lenient::optional")]
  pub upload_concurrency: Option<usize>,
  /// Largest side any output may have. Requests asking for more are rejected.
  /// Defaults to 4096.
  #[serde(default, deserialize_with = "lenient::optional")]
  pub max_output_dimension: Option<i32>,
  /// JPEG quality for the public scale endpoint. Defaults to 80.
  #[serde(default, deserialize_with = "lenient::optional")]
  pub scale_quality: Option<i32>,
  /// Cache-Control header sent with scale endpoint responses.
  /// Defaults to "public, max-age=31536000, immutable".
  pub scale_cache_control: Option<String>,
  /// Seconds a request may run before it is abandoned with 408. Defaults to 60.
  #[serde(default, deserialize_with = "lenient::optional")]
  pub request_timeout_secs: Option<u64>,
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
  /// Static credentials. Leave both unset to use the AWS default credential
  /// chain (environment, shared config, IAM roles, IRSA and so on).
  pub access_key_id: Option<String>,
  pub secret_access_key: Option<String>,
  pub region: String,
  #[serde(deserialize_with = "lenient::required")]
  pub force_path_style: bool,
  pub base_url: String,
}

#[derive(Deserialize)]
pub struct StorageConfigLocal {
  pub path: String,
}

pub fn parse(config_path: &str) -> Result<Config> {
  let toml_str = fs::read_to_string(config_path)
    .with_context(|| format!("failed to read config file: {}", config_path))?;

  parse_with_env(&toml_str, std::env::vars())
}

/// Parse the TOML and then apply overrides from `env`, so secrets and per
/// deployment values never have to live in the file.
///
/// Overrides are always inserted as strings. Numeric and boolean settings
/// accept a string that parses to their type, string settings take the value
/// verbatim, so an API key such as `0123` is never turned into a number.
pub fn parse_with_env<I>(toml_str: &str, env: I) -> Result<Config>
where
  I: IntoIterator<Item = (String, String)>,
{
  let mut table: toml::Table = toml::from_str(toml_str).context("failed to parse config")?;

  for (key, value) in env {
    let Some(path) = key.strip_prefix(ENV_PREFIX) else {
      continue;
    };

    let segments: Vec<String> = path.split("__").map(|s| s.to_lowercase()).collect();
    if segments.iter().any(|s| s.is_empty()) {
      continue;
    }

    set_path(&mut table, &segments, toml::Value::String(value));
  }

  let cfg: Config = table.try_into().context("failed to deserialize config")?;

  Ok(cfg)
}

fn set_path(table: &mut toml::Table, segments: &[String], value: toml::Value) {
  let (last, parents) = segments.split_last().expect("segments are never empty");

  let mut current = table;
  for segment in parents {
    let entry = current
      .entry(segment.clone())
      .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if !entry.is_table() {
      *entry = toml::Value::Table(toml::Table::new());
    }

    current = entry.as_table_mut().expect("just made this a table");
  }

  current.insert(last.clone(), value);
}

/// Deserializers for settings that may come from the TOML file as their native
/// type or from an environment override as a string.
mod lenient {
  use serde::{Deserialize, Deserializer, de::Error};
  use std::{fmt::Display, str::FromStr};

  #[derive(Deserialize)]
  #[serde(untagged)]
  enum Raw<T> {
    Value(T),
    Text(String),
  }

  impl<T> Raw<T>
  where
    T: FromStr,
    T::Err: Display,
  {
    fn into_value<E: Error>(self) -> Result<T, E> {
      match self {
        Raw::Value(v) => Ok(v),
        Raw::Text(s) => s
          .trim()
          .parse()
          .map_err(|e| E::custom(format!("invalid value {:?}: {}", s, e))),
      }
    }
  }

  pub fn required<'de, D, T>(deserializer: D) -> Result<T, D::Error>
  where
    D: Deserializer<'de>,
    T: Deserialize<'de> + FromStr,
    T::Err: Display,
  {
    Raw::<T>::deserialize(deserializer)?.into_value()
  }

  pub fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
  where
    D: Deserializer<'de>,
    T: Deserialize<'de> + FromStr,
    T::Err: Display,
  {
    Option::<Raw<T>>::deserialize(deserializer)?
      .map(Raw::into_value)
      .transpose()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const BASE: &str = r#"
[app]
listen = "0.0.0.0:6100"
metrics_listen = "0.0.0.0:6101"
vips_concurrency = 4
max_body_size_mb = 100
api_key = "file-key"

[storage]
storage_type = "S3"

[storage.s3]
bucket = "bucket"
endpoint = "http://localhost:9008"
base_url = "http://localhost:9008/bucket"
force_path_style = true
region = "auto"
"#;

  fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
      .iter()
      .map(|(k, v)| (k.to_string(), v.to_string()))
      .collect()
  }

  #[test]
  fn env_overrides_nested_values() {
    let env = env(&[
      ("RP__APP__API_KEY", "env-key"),
      ("RP__APP__VIPS_CONCURRENCY", "2"),
      ("RP__APP__ENABLE_OPENAPI", "true"),
      ("RP__APP__WORKER_THREADS", "6"),
      ("RP__STORAGE__S3__FORCE_PATH_STYLE", "false"),
      ("RP__STORAGE__S3__SECRET_ACCESS_KEY", "s3cr3t"),
      ("UNRELATED", "x"),
    ]);

    let cfg = parse_with_env(BASE, env).unwrap();

    assert_eq!(cfg.app.api_key, "env-key");
    assert_eq!(cfg.app.vips_concurrency, 2);
    assert_eq!(cfg.app.enable_openapi, Some(true));
    assert_eq!(cfg.app.worker_threads, Some(6));
    let s3 = cfg.storage.s3.unwrap();
    assert!(!s3.force_path_style);
    assert_eq!(s3.secret_access_key.as_deref(), Some("s3cr3t"));
  }

  #[test]
  fn env_strings_are_never_coerced() {
    let env = env(&[
      ("RP__APP__API_KEY", "0123"),
      ("RP__STORAGE__S3__BUCKET", "2024"),
      ("RP__STORAGE__S3__ACCESS_KEY_ID", "true"),
      ("RP__STORAGE__S3__SECRET_ACCESS_KEY", "42"),
    ]);

    let cfg = parse_with_env(BASE, env).unwrap();

    assert_eq!(cfg.app.api_key, "0123");
    let s3 = cfg.storage.s3.unwrap();
    assert_eq!(s3.bucket, "2024");
    assert_eq!(s3.access_key_id.as_deref(), Some("true"));
    assert_eq!(s3.secret_access_key.as_deref(), Some("42"));
  }

  #[test]
  fn invalid_numeric_override_is_an_error() {
    let env = env(&[("RP__APP__VIPS_CONCURRENCY", "many")]);
    assert!(parse_with_env(BASE, env).is_err());
  }

  #[test]
  fn credentials_are_optional() {
    let cfg = parse_with_env(BASE, Vec::new()).unwrap();
    let s3 = cfg.storage.s3.unwrap();
    assert!(s3.access_key_id.is_none());
    assert!(s3.secret_access_key.is_none());
  }
}
