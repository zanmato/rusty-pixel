use std::path::PathBuf;

use crate::http::storage::{ImageType, PutObjectOutput, Storage};
use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::io::AsyncReadExt;

pub struct Client {
  path: PathBuf,
}

impl Client {
  pub fn new(path: PathBuf) -> Self {
    Self { path }
  }
}

#[async_trait]
impl Storage for Client {
  async fn download_object(&self, key: &str) -> Result<Vec<u8>> {
    let file_path = &self.path.join(key);

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

  async fn upload_object(
    &self,
    data: Vec<u8>,
    key: &str,
    _mime: &str,
    _image_type: ImageType,
  ) -> Result<PutObjectOutput> {
    let size = data.len() as u64;

    let file_path = &self.path.join(key);

    tokio::fs::create_dir_all(file_path.parent().unwrap())
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
