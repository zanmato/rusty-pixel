use axum::{
  http::StatusCode,
  response::{IntoResponse, Response},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
  #[error("bad request {0}")]
  BadRequest(String),
  #[error("not found")]
  NotFound,
  #[error("internal server error {0}")]
  InternalServerError(String),
}

impl IntoResponse for AppError {
  fn into_response(self) -> Response {
    match self {
      AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
      AppError::NotFound => (StatusCode::NOT_FOUND, "Not Found").into_response(),
      AppError::InternalServerError(_msg) => {
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
      }
    }
  }
}
