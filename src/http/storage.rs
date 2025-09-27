use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, Copy)]
pub enum ImageType {
  Original,
  Processed,
}

pub struct PutObjectOutput {
  pub etag: String,
  pub url: String,
  pub size: u64,
}

#[async_trait]
pub trait Storage: Send + Sync {
  async fn download_object(&self, key: &str) -> Result<Vec<u8>>;

  async fn upload_object(
    &self,
    data: Vec<u8>,
    key: &str,
    mime: &str,
    image_type: ImageType,
  ) -> Result<PutObjectOutput>;
}
