use axum::{
  body::Body,
  http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use rusty_pixel::config;
use tokio::sync::OnceCell;
use tokio::{fs, net::TcpListener};
use tower::ServiceExt;

static TEST_BOOSTRAP: OnceCell<axum::Router> = OnceCell::const_new();

async fn bootstrap() -> &'static axum::Router {
  TEST_BOOSTRAP
    .get_or_init(|| async {
      let cfg = config::Config {
        app: config::AppConfig {
          api_key: "test".to_string(),
          vips_concurrency: 1,
          vips_cache_max_mem_mb: 0,
          max_body_size_mb: 10,
          enable_openapi: Some(false),
          worker_threads: None,
          upload_concurrency: None,
          max_output_dimension: None,
          scale_quality: None,
          scale_cache_control: None,
          request_timeout_secs: None,
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

      rusty_pixel::http::bootstrap(&cfg)
        .await
        .expect("failed creating router")
        .router
    })
    .await
}

#[tokio::test]
async fn scale_image() {
  let router = bootstrap().await.clone();

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
  let router = bootstrap().await.clone();

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

  let body = response.text().await.expect("failed to read response");
  let body: serde_json::Value = serde_json::from_str(&body).expect("failed to parse response");
  let images = body
    .as_array()
    .expect("expected an array of processed images");
  assert!(!images.is_empty(), "expected at least one processed image");
  for image in images {
    assert!(
      image["width"].as_i64().unwrap_or(0) > 0,
      "expected non-zero width for {image:?}"
    );
    assert!(
      image["height"].as_i64().unwrap_or(0) > 0,
      "expected non-zero height for {image:?}"
    );
  }

  // A PNG upload should produce a WebP alternative for each configuration,
  // linked back to its primary image via `alternative_to`.
  let primary_ids: Vec<&str> = images
    .iter()
    .filter(|image| image["alternative_to"].is_null())
    .filter_map(|image| image["id"].as_str())
    .collect();

  let alternatives: Vec<&serde_json::Value> = images
    .iter()
    .filter(|image| !image["alternative_to"].is_null())
    .collect();

  assert_eq!(
    alternatives.len(),
    2,
    "expected one WebP alternative per configuration, got {alternatives:?}"
  );

  for alternative in alternatives {
    assert_eq!(
      alternative["mime"].as_str(),
      Some("image/webp"),
      "expected alternative to be a WebP for {alternative:?}"
    );
    assert!(
      alternative["path"]
        .as_str()
        .is_some_and(|path| path.ends_with(".webp")),
      "expected alternative path to end with .webp for {alternative:?}"
    );

    let parent = alternative["alternative_to"]
      .as_str()
      .expect("alternative_to should be a string");
    assert!(
      primary_ids.contains(&parent),
      "alternative_to {parent:?} does not reference a primary image id in {primary_ids:?}"
    );
  }
}

#[tokio::test]
async fn scale_image_converts_to_srgb() {
  let router = bootstrap().await.clone();

  // Solid sRGB red stored as Display P3 with the profile embedded. Its raw
  // pixel values are roughly (234, 51, 35). The orientation option is a no op
  // on this landscape image, so no thumbnail converts the colours on the way.
  let response = router
    .oneshot(
      Request::builder()
        .uri("/scale/olandscape/red-p3.jpg")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);

  let body = response.into_body().collect().await.unwrap().to_bytes();
  let image = libvips::VipsImage::new_from_buffer(&body, "").unwrap();
  let pixel = libvips::ops::getpoint(&image, 32, 24).unwrap();
  let expected = [255.0, 0.0, 0.0];
  for (got, want) in pixel.iter().zip(expected) {
    assert!(
      (got - want).abs() <= 6.0,
      "expected roughly {:?}, got {:?}",
      expected,
      pixel
    );
  }
}

#[tokio::test]
async fn scale_image_not_found() {
  let router = bootstrap().await.clone();

  let response = router
    .oneshot(
      Request::builder()
        .uri("/scale/s400x400/nonexistent.png")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn scale_image_invalid_options() {
  let router = bootstrap().await.clone();

  let response = router
    .oneshot(
      Request::builder()
        .uri("/scale/invalid/skaune-portrait.png")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  // Invalid options are rejected before storage is touched
  assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn process_image_unauthorized() {
  let router = bootstrap().await.clone();

  let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
  let addr = listener.local_addr().unwrap();

  tokio::spawn(async move {
    axum::serve(listener, router).await.unwrap();
  });

  let client = reqwest::Client::new();

  // No API key
  let response = client
    .post(&format!(
      "http://{}:{}/api/v1/process-image",
      addr.ip(),
      addr.port()
    ))
    .send()
    .await
    .expect("failed to send request");

  assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

  // Wrong API key
  let response = client
    .post(&format!(
      "http://{}:{}/api/v1/process-image",
      addr.ip(),
      addr.port()
    ))
    .header("X-API-Key", "wrong-key")
    .send()
    .await
    .expect("failed to send request");

  assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn process_image_missing_fields() {
  let router = bootstrap().await.clone();

  let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
  let addr = listener.local_addr().unwrap();

  tokio::spawn(async move {
    axum::serve(listener, router).await.unwrap();
  });

  let client = reqwest::Client::new();

  // Only send image, no details field
  let file = fs::read("tests/testdata/skaune-portrait.png")
    .await
    .expect("failed to read file");
  let file_part = reqwest::multipart::Part::bytes(file)
    .file_name("skaune-portrait.png")
    .mime_str("image/png")
    .unwrap();

  let form = reqwest::multipart::Form::new().part("image", file_part);

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

  assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn process_svg_without_allow_vector() {
  let router = bootstrap().await.clone();

  let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
  let addr = listener.local_addr().unwrap();

  tokio::spawn(async move {
    axum::serve(listener, router).await.unwrap();
  });

  // An SVG uploaded without `allow_vector` should be rasterized and still
  // produce a WebP alternative for the configuration.
  let json_request = r#"{
    "id": "svg-original",
    "path": "output",
    "save_original": false,
    "generate_alternative": false,
    "max_age": 31536000,
    "configurations": [
      {
        "id": "svg-config",
        "path": "output_svg",
        "aspect": 1.33,
        "margin_percent": 10,
        "size": 1024,
        "quality": 80,
        "conditions": {
          "use_original_mime": false,
          "allow_vector": false,
          "transparent": false,
          "trim": false,
          "black_and_white": false,
          "option_id": "uuid",
          "use_environment_image": false
        }
      }
    ]
  }"#;

  let json_part = reqwest::multipart::Part::text(json_request);

  let file = fs::read("tests/testdata/skaune.svg")
    .await
    .expect("failed to read file");
  let file_part = reqwest::multipart::Part::bytes(file)
    .file_name("skaune.svg")
    .mime_str("image/svg+xml")
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

  let body = response.text().await.expect("failed to read response");
  let body: serde_json::Value = serde_json::from_str(&body).expect("failed to parse response");
  let images = body
    .as_array()
    .expect("expected an array of processed images");
  assert!(!images.is_empty(), "expected at least one processed image");

  // The main image must be rasterized, not passed through as SVG, since
  // allow_vector is false.
  let primaries: Vec<&serde_json::Value> = images
    .iter()
    .filter(|image| image["alternative_to"].is_null())
    .collect();

  assert_eq!(
    primaries.len(),
    1,
    "expected exactly one primary image, got {primaries:?}"
  );

  for primary in &primaries {
    assert_ne!(
      primary["mime"].as_str(),
      Some("image/svg+xml"),
      "expected main image to be rasterized when allow_vector is false, got {primary:?}"
    );
    assert_eq!(
      primary["mime"].as_str(),
      Some("image/jpeg"),
      "expected main image to be a JPEG when allow_vector is false, got {primary:?}"
    );
    assert!(
      primary["path"]
        .as_str()
        .is_some_and(|path| !path.ends_with(".svg")),
      "expected main image path to not end with .svg for {primary:?}"
    );
  }

  let primary_ids: Vec<&str> = primaries
    .iter()
    .filter_map(|image| image["id"].as_str())
    .collect();

  let alternatives: Vec<&serde_json::Value> = images
    .iter()
    .filter(|image| !image["alternative_to"].is_null())
    .collect();

  assert_eq!(
    alternatives.len(),
    1,
    "expected one WebP alternative for the SVG configuration, got {alternatives:?}"
  );

  for alternative in alternatives {
    assert_eq!(
      alternative["mime"].as_str(),
      Some("image/webp"),
      "expected alternative to be a WebP for {alternative:?}"
    );
    assert!(
      alternative["path"]
        .as_str()
        .is_some_and(|path| path.ends_with(".webp")),
      "expected alternative path to end with .webp for {alternative:?}"
    );

    let parent = alternative["alternative_to"]
      .as_str()
      .expect("alternative_to should be a string");
    assert!(
      primary_ids.contains(&parent),
      "alternative_to {parent:?} does not reference a primary image id in {primary_ids:?}"
    );
  }
}

/// Post one image with its processing details and return the response.
async fn post_process(details: &str, fixture: &str, mime: &str) -> reqwest::Response {
  let router = bootstrap().await.clone();

  let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
  let addr = listener.local_addr().unwrap();

  tokio::spawn(async move {
    axum::serve(listener, router).await.unwrap();
  });

  let file = fs::read(format!("tests/testdata/{fixture}"))
    .await
    .expect("failed to read file");
  let file_part = reqwest::multipart::Part::bytes(file)
    .file_name(fixture.to_owned())
    .mime_str(mime)
    .unwrap();

  let form = reqwest::multipart::Form::new()
    .part("image", file_part)
    .part(
      "details",
      reqwest::multipart::Part::text(details.to_owned()),
    );

  reqwest::Client::new()
    .post(format!(
      "http://{}:{}/api/v1/process-image",
      addr.ip(),
      addr.port()
    ))
    .header("X-API-Key", "test")
    .multipart(form)
    .send()
    .await
    .expect("failed to send request")
}

/// A configuration in the shape of the featured image: source aspect, no
/// margin, longest side 1024.
fn featured_details(path: &str, min_side: &str, transparent: bool) -> String {
  format!(
    r#"{{
      "id": "original",
      "path": "generated/{path}_original",
      "save_original": false,
      "configurations": [
        {{
          "id": "featured",
          "path": "generated/{path}",
          "aspect": 0.0,
          "margin_percent": 0,
          "size": 1024,
          {min_side}
          "quality": 80,
          "conditions": {{
            "allow_vector": false,
            "transparent": {transparent},
            "trim": false,
            "black_and_white": false,
            "use_environment_image": false
          }}
        }}
      ]
    }}"#
  )
}

fn pixel(path: &str, x: i32, y: i32) -> Vec<f64> {
  let image = libvips::VipsImage::new_from_file(&format!("tests/testdata/{path}"))
    .expect("failed to open output");
  libvips::ops::getpoint(&image, x, y).expect("failed to read pixel")
}

fn assert_close(actual: &[f64], expected: &[f64], context: &str) {
  assert_eq!(actual.len(), expected.len(), "{context}: got {actual:?}");
  for (a, e) in actual.iter().zip(expected) {
    assert!(
      (a - e).abs() <= 6.0,
      "{context}: expected about {expected:?}, got {actual:?}"
    );
  }
}

async fn dimensions(response: reqwest::Response) -> Vec<(String, i64, i64)> {
  assert_eq!(response.status(), reqwest::StatusCode::OK);
  let body = response.text().await.expect("failed to read response");
  let body: serde_json::Value = serde_json::from_str(&body).expect("failed to parse response");
  body
    .as_array()
    .expect("expected an array")
    .iter()
    .map(|image| {
      (
        image["mime"].as_str().unwrap().to_owned(),
        image["width"].as_i64().unwrap(),
        image["height"].as_i64().unwrap(),
      )
    })
    .collect()
}

/// Without `min_side` a wide source keeps its short side, this is the
/// behaviour existing configurations rely on.
#[tokio::test]
async fn process_image_without_min_side_keeps_source_shape() {
  let details = featured_details("no_min_side", "", false);
  let outputs = dimensions(post_process(&details, "wide-blue.png", "image/png").await).await;

  assert_eq!(outputs.len(), 2);
  for (mime, width, height) in outputs {
    assert_eq!((width, height), (1024, 307), "{mime}");
  }
}

/// With `min_side` the short side is padded with the colour of the image's
/// border and the image is not stretched.
#[tokio::test]
async fn process_image_min_side_pads_with_border_colour() {
  let details = featured_details("min_side_wide", r#""min_side": 500,"#, false);
  let outputs = dimensions(post_process(&details, "wide-blue.png", "image/png").await).await;

  assert_eq!(outputs.len(), 2);
  for (mime, width, height) in outputs {
    assert_eq!((width, height), (1024, 500), "{mime}");
  }

  let blue = [40.0, 90.0, 160.0];
  for file in [
    "generated/min_side_wide.jpg",
    "generated/min_side_wide.webp",
  ] {
    // Padding above and below, and the image's own background in the middle
    assert_close(&pixel(file, 5, 5), &blue, file);
    assert_close(&pixel(file, 1018, 495), &blue, file);
    assert_close(&pixel(file, 5, 250), &blue, file);
    // The circle is still in the centre and still round: 80 source pixels
    // across scale to 205, so it must not reach 110 pixels from the centre
    assert_close(&pixel(file, 512, 250), &[255.0, 0.0, 0.0], file);
    assert_close(&pixel(file, 512 + 95, 250), &[255.0, 0.0, 0.0], file);
    assert_close(&pixel(file, 512 + 110, 250), &blue, file);
    assert_close(&pixel(file, 512, 250 + 95), &[255.0, 0.0, 0.0], file);
    assert_close(&pixel(file, 512, 250 + 110), &blue, file);
  }
}

/// The padding colour comes from the corners. A product that fills its
/// picture, like a phone case with a dark rounded frame, covers most of the
/// border and must not decide the colour.
#[tokio::test]
async fn process_image_min_side_samples_corners_not_the_frame() {
  let details = featured_details("min_side_framed", r#""min_side": 500,"#, false);
  let outputs = dimensions(post_process(&details, "wide-framed.png", "image/png").await).await;

  assert_eq!(outputs[0], ("image/jpeg".to_owned(), 1024, 500));

  let file = "generated/min_side_framed.jpg";
  let white = [255.0, 255.0, 255.0];
  assert_close(&pixel(file, 5, 5), &white, "padding above");
  assert_close(&pixel(file, 512, 490), &white, "padding below");
  // The frame itself is still black, just inside the top edge of the image
  assert_close(&pixel(file, 512, 110), &[0.0, 0.0, 0.0], "frame");
}

/// An image close to the minimum is scaled up instead of padded, one that is
/// far from it is still padded at the normal size.
#[tokio::test]
async fn process_image_min_side_scales_when_close() {
  let options = r#""min_side": 500, "min_side_max_scale": 1.15,"#;

  // 400 x 180 is 460 high at 1024, 9 percent short
  let details = featured_details("min_side_scaled", options, false);
  let outputs = dimensions(post_process(&details, "wide-near.png", "image/png").await).await;
  assert_eq!(outputs.len(), 2);
  for (mime, width, height) in outputs {
    assert_eq!((width, height), (1112, 500), "{mime}");
  }

  // Not padded: the circle is 120 of 180 source pixels high, so it reaches
  // 166 pixels from the centre, which it would not at the 1024 scale
  let file = "generated/min_side_scaled.jpg";
  assert_close(&pixel(file, 556, 250 + 160), &[255.0, 0.0, 0.0], file);
  assert_close(&pixel(file, 556, 250 + 190), &[40.0, 90.0, 160.0], file);

  // 400 x 120 is 307 high at 1024, far too short to scale
  let details = featured_details("min_side_scale_too_far", options, false);
  let outputs = dimensions(post_process(&details, "wide-blue.png", "image/png").await).await;
  assert_eq!(outputs[0], ("image/jpeg".to_owned(), 1024, 500));
}

#[tokio::test]
async fn process_image_rejects_invalid_min_side_max_scale() {
  for options in [
    r#""min_side": 500, "min_side_max_scale": 0.5,"#,
    r#""min_side": 500, "min_side_max_scale": 10.0,"#,
  ] {
    let details = featured_details("min_side_scale_invalid", options, false);
    let response = post_process(&details, "wide-blue.png", "image/png").await;
    assert_eq!(
      response.status(),
      reqwest::StatusCode::BAD_REQUEST,
      "{options}"
    );
  }
}

/// A transparent image must be padded with transparency, never with an
/// opaque frame.
#[tokio::test]
async fn process_image_min_side_keeps_transparency() {
  let details = featured_details("min_side_transparent", r#""min_side": 500,"#, true);
  let outputs = dimensions(post_process(&details, "wide-transparent.png", "image/png").await).await;

  assert_eq!(outputs[0], ("image/png".to_owned(), 1024, 500));

  for file in [
    "generated/min_side_transparent.png",
    "generated/min_side_transparent.webp",
  ] {
    assert_eq!(
      pixel(file, 5, 5)[3],
      0.0,
      "{file} padding should be transparent"
    );
    assert_eq!(pixel(file, 5, 250)[3], 0.0, "{file} image background");
    assert_eq!(pixel(file, 512, 250)[3], 255.0, "{file} circle");
  }
}

/// The frame added for an aspect ratio or margin must be transparent too.
#[tokio::test]
async fn process_image_frame_keeps_transparency() {
  let details = featured_details("frame_transparent", "", true)
    .replace(r#""aspect": 0.0"#, r#""aspect": 1.0"#)
    .replace(r#""margin_percent": 0"#, r#""margin_percent": 10"#);
  let outputs = dimensions(post_process(&details, "wide-transparent.png", "image/png").await).await;

  assert_eq!(outputs[0], ("image/png".to_owned(), 1024, 1024));

  for file in [
    "generated/frame_transparent.png",
    "generated/frame_transparent.webp",
  ] {
    assert_eq!(
      pixel(file, 5, 5)[3],
      0.0,
      "{file} frame should be transparent"
    );
    assert_eq!(pixel(file, 512, 512)[3], 255.0, "{file} circle");
  }
}

/// An opaque image still gets a white frame for its aspect ratio and margin.
#[tokio::test]
async fn process_image_frame_is_white_for_opaque_images() {
  let details = featured_details("frame_opaque", "", false)
    .replace(r#""aspect": 0.0"#, r#""aspect": 1.0"#)
    .replace(r#""margin_percent": 0"#, r#""margin_percent": 10"#);
  let outputs = dimensions(post_process(&details, "wide-blue.png", "image/png").await).await;

  assert_eq!(outputs[0], ("image/jpeg".to_owned(), 1024, 1024));
  assert_close(
    &pixel("generated/frame_opaque.jpg", 5, 5),
    &[255.0, 255.0, 255.0],
    "frame",
  );
}

#[tokio::test]
async fn process_image_rejects_min_side_above_size() {
  let details = featured_details("min_side_invalid", r#""min_side": 2000,"#, false);
  let response = post_process(&details, "wide-blue.png", "image/png").await;

  assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}
