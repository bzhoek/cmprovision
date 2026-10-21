use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::pages::{cms, dashboard, firmware, images, labels, profile, projects, scripts, settings};
use crate::{AppState, auth, provision, session};

/// Provisioning logs are truncated to 1 MiB after upload; leave room for multipart overhead.
const SCRIPTEXECUTE_BODY_LIMIT: usize = 16 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    let web = Router::new()
        .route("/", get(auth::home))
        .route("/login", get(auth::login_form).post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/two-factor-challenge", get(auth::challenge_form).post(auth::challenge))
        .route("/dashboard", get(dashboard::show))
        .route("/user/profile", get(profile::show))
        .route("/user/profile-information", post(profile::update_profile))
        .route("/user/password", post(profile::update_password))
        .route("/user/two-factor/enable", post(profile::enable_two_factor))
        .route("/user/two-factor/confirm", post(profile::confirm_two_factor))
        .route("/user/two-factor/cancel", post(profile::cancel_two_factor))
        .route("/user/two-factor/disable", post(profile::disable_two_factor))
        .route("/user/two-factor/recovery-codes", post(profile::regenerate_recovery_codes))
        .route("/user/api-tokens", get(profile::api_tokens).post(profile::create_api_token))
        .route("/user/api-tokens/{id}", post(profile::update_api_token))
        .route("/user/api-tokens/{id}/delete", post(profile::delete_api_token))
        .route("/cms", get(cms::index))
        .route("/cms/export.csv", get(cms::export))
        .route("/cms/{id}", get(cms::show))
        .route("/cms/{id}/delete", post(cms::delete))
        .route("/images", get(images::index))
        // Images are uploaded as multi-gigabyte request bodies, streamed straight to disk.
        .route("/addImage", post(images::upload).layer(DefaultBodyLimit::disable()))
        .route("/images/{id}/delete", post(images::delete))
        .route("/projects", get(projects::index).post(projects::create))
        .route("/projects/new", get(projects::new))
        .route("/projects/{id}", post(projects::update))
        .route("/projects/{id}/edit", get(projects::edit))
        .route("/projects/{id}/activate", post(projects::set_active))
        .route("/projects/{id}/delete", post(projects::delete))
        .route("/scripts", get(scripts::index).post(scripts::create))
        .route("/scripts/new", get(scripts::new))
        .route("/scripts/{id}", post(scripts::update))
        .route("/scripts/{id}/edit", get(scripts::edit))
        .route("/scripts/{id}/delete", post(scripts::delete))
        .route("/labels", get(labels::index).post(labels::create))
        .route("/labels/new", get(labels::new))
        .route("/labels/test", post(labels::test_print))
        .route("/labels/{id}", post(labels::update))
        .route("/labels/{id}/edit", get(labels::edit))
        .route("/labels/{id}/delete", post(labels::delete))
        .route("/firmware", get(firmware::index))
        .route("/firmware/update", post(firmware::update))
        .route("/firmware/bootconf", get(projects::bootconf))
        .route("/settings", get(settings::index))
        .route("/settings/log/{service}", get(settings::log))
        .route("/settings/static-ips", post(settings::create_static_ip))
        .route("/settings/static-ips/new", get(settings::new_static_ip))
        .route("/settings/static-ips/{id}/delete", post(settings::delete_static_ip))
        .layer(session::layer(state.db.clone()))
        .layer(middleware::from_fn(auth::same_origin));

    Router::new()
        .merge(web)
        .route("/healthz", get(healthz))
        .route("/assets/app.css", get(stylesheet))
        .route("/favicon.ico", get(favicon))
        .route("/scriptexecute", any(provision::scriptexecute).layer(DefaultBodyLimit::max(SCRIPTEXECUTE_BODY_LIMIT)))
        // Unauthenticated, like nginx served public/uploads: devices download images and
        // pieeprom.bin from here, and the random 40-character names are the only protection.
        .nest_service("/uploads", ServeDir::new(state.config.uploads_dir()))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn stylesheet() -> Response {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("../assets/dist/app.css")).into_response()
}

async fn favicon() -> Response {
    ([(header::CONTENT_TYPE, "image/x-icon")], include_bytes!("../public/favicon.ico").as_slice()).into_response()
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
