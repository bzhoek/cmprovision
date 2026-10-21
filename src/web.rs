use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use tower_http::trace::TraceLayer;

use crate::{AppState, provision};

/// Provisioning logs are truncated to 1 MiB after upload; leave room for multipart overhead.
const SCRIPTEXECUTE_BODY_LIMIT: usize = 16 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/scriptexecute", any(provision::scriptexecute).layer(DefaultBodyLimit::max(SCRIPTEXECUTE_BODY_LIMIT)))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

#[derive(Debug)]
pub enum Error {
    Status(StatusCode),
    Internal(anyhow::Error),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Error::Status(status) => (status, html(status.canonical_reason().unwrap_or_default())).into_response(),
            Error::Internal(err) => {
                tracing::error!("{err:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, html("Server Error")).into_response()
            }
        }
    }
}

impl<E: Into<anyhow::Error>> From<E> for Error {
    fn from(err: E) -> Self {
        Error::Internal(err.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn html(body: impl Into<String>) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=UTF-8")], body.into()).into_response()
}

pub fn text(body: impl Into<String>) -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=UTF-8")], body.into()).into_response()
}
