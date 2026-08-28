use std::path::{Component, Path, PathBuf};

use crate::http::storage::{PutObjectOutput, Storage};
use anyhow::{Context, Result};
use async_trait::async_trait;
use axum::body::Bytes;
use tokio::io::AsyncReadExt;

pub struct Client {
  path: PathBuf,
}

impl Client {
  pub fn new(path: PathBuf) -> Self {
    Self { path }
  }

  /// Resolve a key inside the storage root, rejecting anything that would
  /// escape it. Keys come straight from the URL on the public endpoint.
  fn resolve(&self, key: &str) -> Result<PathBuf> {
    let relative = Path::new(key.trim_start_matches('/'));

    if relative
      .components()
      .any(|c| !matches!(c, Component::Normal(_)))
    {
      anyhow::bail!("invalid key: {}", key);
    }

    Ok(self.path.join(relative))
  }
}

#[async_trait]
impl Storage for Client {
  async fn download_object(&self, key: &str) -> Result<Vec<u8>> {
    let file_path = self.resolve(key)?;

    let mut file = tokio::fs::File::open(&file_path)
      .await
      .with_context(|| format!("failed to open file: {}", key))?;

    let mut data = Vec::new();
    file
      .read_to_end(&mut data)
      .await
      .with_context(|| format!("failed to read file: {}", key))?;

    Ok(data)
  }

  async fn upload_object(&self, data: Bytes, key: &str, _mime: &str) -> Result<PutObjectOutput> {
    let size = data.len() as u64;

    let file_path = self.resolve(key)?;

    tokio::fs::create_dir_all(
      file_path
        .parent()
        .with_context(|| format!("invalid file path has no parent: {}", key))?,
    )
    .await
    .with_context(|| format!("failed to create directory: {}", key))?;

    tokio::fs::write(&file_path, &data)
      .await
      .with_context(|| format!("failed to write file: {}", key))?;

    Ok(PutObjectOutput {
      etag: "".to_owned(),
      url: "".to_owned(),
      size,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn resolve_rejects_traversal() {
    let client = Client::new(PathBuf::from("/data"));
    assert!(client.resolve("../etc/passwd").is_err());
    assert!(client.resolve("a/../../b").is_err());
    assert!(client.resolve("/abs/../x").is_err());
  }

  #[test]
  fn resolve_accepts_nested_keys() {
    let client = Client::new(PathBuf::from("/data"));
    assert_eq!(
      client.resolve("/images/a/b.png").unwrap(),
      PathBuf::from("/data/images/a/b.png")
    );
  }
}
