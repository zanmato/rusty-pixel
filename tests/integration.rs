use axum::{
  body::Body,
  http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use rusty_pixel::config;
use std::sync::OnceLock;
use tokio::{fs, net::TcpListener};
use tower::ServiceExt;

static TEST_BOOSTRAP: OnceLock<axum::Router> = OnceLock::new();

fn bootstrap() -> &'static axum::Router {
  let router = TEST_BOOSTRAP.get_or_init(|| {
    let cfg = config::Config {
      app: config::AppConfig {
        api_key: "test".to_string(),
        vips_concurrency: 1,
        max_body_size_mb: 10,
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

    rusty_pixel::http::bootstrap(&cfg).expect("failed creating router")
  });

  router
}

#[tokio::test]
async fn scale_image() {
  let router = bootstrap().clone();

  let response = router
    .oneshot(
      Request::builder()
        .uri("/scale/bw-olandscape-s400x400-m20/skaune-portrait.png")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);

  // Save the image to disk
  let body = response.into_body().collect().await.unwrap().to_bytes();
  fs::write("tests/output/scale.png", body)
    .await
    .expect("failed saving image");
}

#[tokio::test]
async fn process_image() {
  let router = bootstrap().clone();

  let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
  let addr = listener.local_addr().unwrap();

  tokio::spawn(async move {
    axum::serve(listener, router).await.unwrap();
  });

  let json_request = r#"{
    "id": "originale",
    "path": "output",
    "save_original": true,
    "generate_alternative": false,
    "max_age": 31536000,
    "portrait_environment_image": {
      "path": "env.png",
      "width": 172,
      "height": 235,
      "x": 164,
      "y": 32,
      "margin_percent": 20
    },
    "configurations": [
      {
        "id": "testar1",
        "path": "output_noenv",
        "aspect": 1.33,
        "margin_percent": 10,
        "size": 1024,
        "quality": 80,
        "conditions": {
          "use_original_mime": true,
          "allow_vector": true,
          "transparent": true,
          "trim": true,
          "black_and_white": true,
          "option_id": "uuid",
          "use_environment_image": false
        }
      },
      {
        "id": "testar2",
        "path": "output_env",
        "aspect": 1.33,
        "margin_percent": 10,
        "size": 1024,
        "quality": 80,
        "conditions": {
          "use_original_mime": true,
          "allow_vector": true,
          "mime": "image/jpeg",
          "transparent": true,
          "trim": true,
          "black_and_white": true,
          "option_id": "uuid",
          "use_environment_image": true
        }
      }
    ]
  }"#;

  let json_part = reqwest::multipart::Part::text(json_request);

  let file = fs::read("tests/testdata/skaune-portrait.png")
    .await
    .expect("failed to read file");
  let file_part = reqwest::multipart::Part::bytes(file)
    .file_name("skaune-portrait.png")
    .mime_str("image/png")
    .unwrap();

  let form = reqwest::multipart::Form::new()
    .part("image", file_part)
    .part("details", json_part);

  let client = reqwest::Client::new();

  let response = client
    .post(&format!(
      "http://{}:{}/api/v1/process-image",
      addr.ip(),
      addr.port()
    ))
    .header("X-API-Key", "test")
    .multipart(form)
    .send()
    .await
    .expect("failed to send request");

  assert_eq!(response.status(), reqwest::StatusCode::OK);
}
