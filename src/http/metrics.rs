//! Prometheus metrics for the HTTP layer and the image pipeline.
//!
//! All metric names are defined here so that dashboards have a single place
//! to look, and so that the pipeline code does not have to repeat strings.

use axum::{
  extract::{MatchedPath, Request},
  middleware::Next,
  response::IntoResponse,
};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::{sync::OnceLock, time::Instant};

pub const HTTP_REQUESTS_TOTAL: &str = "http_requests_total";
pub const HTTP_REQUEST_DURATION: &str = "http_requests_duration_seconds";

/// Time between handing work to the rayon pool and the work starting.
pub const QUEUE_WAIT: &str = "image_queue_wait_seconds";
/// Wall time of the whole image pipeline for one request, excluding uploads.
pub const PROCESS_DURATION: &str = "image_process_duration_seconds";
/// Time spent decoding and scaling one configuration.
pub const DECODE_DURATION: &str = "image_decode_duration_seconds";
/// Time spent encoding one output, labelled by format.
pub const ENCODE_DURATION: &str = "image_encode_duration_seconds";
/// Size of one encoded output, labelled by format.
pub const OUTPUT_BYTES: &str = "image_output_bytes";
/// Number of configurations in one process request.
pub const CONFIGURATIONS: &str = "image_configurations_per_request";
/// Time spent uploading one object to storage.
pub const UPLOAD_DURATION: &str = "image_upload_duration_seconds";
pub const UPLOAD_ERRORS: &str = "image_upload_errors_total";
/// Requests abandoned because the client went away or the timeout fired.
pub const CANCELLED: &str = "image_requests_cancelled_total";

/// Buckets that cover fast metadata calls as well as multi configuration
/// process requests that can run for tens of seconds.
const DURATION_SECONDS: &[f64] = &[
  0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0,
];

const BYTES: &[f64] = &[
  1e3, 5e3, 1e4, 2.5e4, 5e4, 1e5, 2.5e5, 5e5, 1e6, 2.5e6, 5e6, 1e7,
];

const COUNTS: &[f64] = &[1.0, 2.0, 3.0, 5.0, 8.0, 12.0, 16.0, 24.0, 32.0];

static RECORDER: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global recorder on first use and hand out its handle. The
/// recorder is process wide, so bootstrapping more than once, as tests do,
/// shares it instead of failing.
pub fn setup_recorder() -> PrometheusHandle {
  RECORDER
    .get_or_init(|| {
      PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Suffix("_seconds".into()), DURATION_SECONDS)
        .unwrap()
        .set_buckets_for_metric(Matcher::Full(OUTPUT_BYTES.into()), BYTES)
        .unwrap()
        .set_buckets_for_metric(Matcher::Full(CONFIGURATIONS.into()), COUNTS)
        .unwrap()
        .install_recorder()
        .expect("failed to install prometheus recorder")
    })
    .clone()
}

/// Records request count and latency per matched route. Unmatched routes are
/// grouped under one label so that scanners cannot blow up the label space.
pub async fn track(req: Request, next: Next) -> impl IntoResponse {
  let start = Instant::now();
  let path = req
    .extensions()
    .get::<MatchedPath>()
    .map(|p| p.as_str().to_owned())
    .unwrap_or_else(|| "unmatched".to_owned());
  let method = req.method().clone();

  let response = next.run(req).await;

  let labels = [
    ("method", method.to_string()),
    ("path", path),
    ("status", response.status().as_u16().to_string()),
  ];

  metrics::counter!(HTTP_REQUESTS_TOTAL, &labels).increment(1);
  metrics::histogram!(HTTP_REQUEST_DURATION, &labels).record(start.elapsed().as_secs_f64());

  response
}

/// Records the elapsed time of a block of code into a histogram.
pub struct Timer {
  start: Instant,
}

impl Timer {
  pub fn start() -> Self {
    Timer {
      start: Instant::now(),
    }
  }

  pub fn observe(&self, name: &'static str) {
    metrics::histogram!(name).record(self.start.elapsed().as_secs_f64());
  }

  pub fn observe_with(&self, name: &'static str, label: &'static str, value: &'static str) {
    metrics::histogram!(name, label => value).record(self.start.elapsed().as_secs_f64());
  }
}
