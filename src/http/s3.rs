use crate::http::storage::{PutObjectOutput, Storage};
use anyhow::{Context, Result};
use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use tokio::io::AsyncReadExt;
use tracing::debug;
use url::Url;

pub struct Client {
  s3_client: aws_sdk_s3::Client,
  bucket: String,
  base_url: Url,
}

impl Client {
  pub fn new(s3_client: aws_sdk_s3::Client, bucket: &str, base_url: &str) -> Self {
    let base_url = Url::parse(base_url).expect("failed to parse base url");

    Self {
      s3_client,
      bucket: bucket.to_owned(),
      base_url,
    }
  }
}

#[async_trait]
impl Storage for Client {
  async fn download_object(&self, key: &str) -> Result<Vec<u8>> {
    let trimmed = key.trim_start_matches('/');

    debug!(
      "downloading object: {} from bucket: {}",
      trimmed, self.bucket
    );

    let object = self
      .s3_client
      .get_object()
      .bucket(self.bucket.as_str())
      .key(trimmed)
      .send()
      .await?;

    let mut data = Vec::with_capacity(object.content_length.unwrap() as usize);
    object.body.into_async_read().read_to_end(&mut data).await?;

    Ok(data)
  }

  async fn upload_object(&self, data: Vec<u8>, key: &str, mime: &str) -> Result<PutObjectOutput> {
    let size = data.len() as u64;
    let body = ByteStream::from(data);
    let res = self
      .s3_client
      .put_object()
      .bucket(self.bucket.as_str())
      .key(key)
      .body(body)
      .cache_control("public, max-age=31536000, immutable".to_owned())
      .content_type(mime)
      .send()
      .await
      .context("failed to upload object");

    let url = self.base_url.join(key)?.to_string();

    Ok(PutObjectOutput {
      etag: res?.e_tag.unwrap_or("".to_owned()).trim_matches('"').into(),
      url,
      size,
    })
  }
}
