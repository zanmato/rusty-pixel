use rusty_pixel::config;
use rusty_pixel::http;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
  // Load config
  let cfg = crate::config::parse("config.toml").expect("failed to parse config");

  // Initialize tracing
  tracing_subscriber::registry()
    .with(
      tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "rusty_pixel=debug,tower_http=debug".into()),
    )
    .with(
      tracing_subscriber::fmt::layer()
        .with_target(false)
        .compact(),
    )
    .init();

  // Serve
  let router = http::bootstrap(&cfg).expect("failed creating router");

  let (_main_server, _metrics_server) = tokio::join!(
    http::serve(router, &cfg.app.listen),
    http::serve_metrics(&cfg.app.metrics_listen),
  );
}
