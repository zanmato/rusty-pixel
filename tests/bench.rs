//! Rough end to end benchmark for the process image endpoint.
//!
//! Ignored by default because it takes a while. Run with:
//!   cargo test --release --test bench -- --ignored --nocapture
//!
//! Posts a large JPEG with eight configurations a number of times and prints
//! p50 and p95 wall time so changes to the pipeline can be compared.

use libvips::{VipsImage, ops};
use rusty_pixel::config;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

const ITERATIONS: usize = 10;
const SOURCE_SIZE: i32 = 4000;

/// Must be called after `bootstrap`, which is what initialises libvips.
fn large_jpeg() -> Vec<u8> {
  let source = std::fs::read("tests/testdata/skaune-portrait.png").unwrap();
  let img = ops::thumbnail_buffer_with_opts(
    &source,
    SOURCE_SIZE,
    &ops::ThumbnailBufferOptions {
      height: SOURCE_SIZE,
      size: ops::Size::Up,
      ..ops::ThumbnailBufferOptions::default()
    },
  )
  .unwrap();
  let flat = ops::flatten(&img).unwrap();
  let data = ops::jpegsave_buffer_with_opts(
    &flat,
    &ops::JpegsaveBufferOptions {
      q: 90,
      ..ops::JpegsaveBufferOptions::default()
    },
  )
  .unwrap();
  // Make sure the image can be opened again before handing it out
  VipsImage::new_from_buffer(&data, "").unwrap();
  data
}

fn request_json() -> String {
  let configurations: Vec<String> = (0..8)
    .map(|i| {
      let size = 256 * (i + 1);
      format!(
        r#"{{
          "id": "bench{i}",
          "path": "output_bench_{i}",
          "aspect": 1.33,
          "margin_percent": 5,
          "size": {size},
          "quality": 80,
          "conditions": {{
            "allow_vector": false,
            "transparent": {transparent},
            "trim": {trim},
            "black_and_white": false,
            "use_environment_image": false
          }}
        }}"#,
        transparent = i % 4 == 0,
        trim = i % 2 == 0,
      )
    })
    .collect();

  format!(
    r#"{{
      "id": "bench",
      "path": "output_bench",
      "save_original": false,
      "configurations": [{}]
    }}"#,
    configurations.join(",")
  )
}

#[tokio::test]
#[ignore]
async fn process_image_throughput() {
  let cfg = config::Config {
    app: config::AppConfig {
      api_key: "test".to_string(),
      vips_concurrency: 1,
      max_body_size_mb: 100,
      enable_openapi: Some(false),
      listen: "0.0.0.0:0".to_string(),
      metrics_listen: "0.0.0.0:0".to_string(),
    },
    storage: config::StorageConfig {
      storage_type: config::StorageType::Local,
      local: Some(config::StorageConfigLocal {
        path: "tests/testdata".to_string(),
      }),
      s3: None,
    },
  };
  let router = rusty_pixel::http::bootstrap(&cfg)
    .expect("failed creating router")
    .router;

  let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
  let addr = listener.local_addr().unwrap();
  tokio::spawn(async move {
    axum::serve(listener, router).await.unwrap();
  });

  let image = large_jpeg();
  println!("source jpeg: {} bytes", image.len());
  let json = request_json();
  let client = reqwest::Client::new();

  let mut samples = Vec::with_capacity(ITERATIONS);
  for _ in 0..ITERATIONS {
    let form = reqwest::multipart::Form::new()
      .part(
        "image",
        reqwest::multipart::Part::bytes(image.clone())
          .file_name("bench.jpg")
          .mime_str("image/jpeg")
          .unwrap(),
      )
      .part("details", reqwest::multipart::Part::text(json.clone()));

    let start = Instant::now();
    let response = client
      .post(format!("http://{}/api/v1/process-image", addr))
      .header("X-API-Key", "test")
      .multipart(form)
      .send()
      .await
      .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    samples.push(start.elapsed());
    assert_eq!(body.as_array().unwrap().len(), 16);
  }

  samples.sort();
  let p = |q: f64| samples[((samples.len() - 1) as f64 * q) as usize];
  let total: Duration = samples.iter().sum();
  println!(
    "process-image x{ITERATIONS}: p50 {:?} p95 {:?} min {:?} mean {:?}",
    p(0.5),
    p(0.95),
    samples[0],
    total / ITERATIONS as u32
  );
}
