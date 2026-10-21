//! Session login for the web interface, with optional TOTP two-factor authentication.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use sqlx::{FromRow, SqlitePool};
use tower_sessions::{Expiry, Session};

use crate::templates::{self, Flash};
use crate::web::{Error, Result};
use crate::{AppState, session, twofactor};

const USER_ID: &str = "auth.user_id";
const PENDING_USER_ID: &str = "auth.pending_user_id";
const PENDING_REMEMBER: &str = "auth.pending_remember";
const INTENDED: &str = "auth.intended";

pub const BAD_CREDENTIALS: &str = "These credentials do not match our records.";
pub const BAD_CODE: &str = "The provided two factor authentication code was invalid.";

#[derive(Clone, Debug, FromRow)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub password: String,
    pub two_factor_secret: Option<String>,
    pub two_factor_recovery_codes: Option<String>,
}

impl User {
    pub fn has_two_factor(&self) -> bool {
        self.two_factor_secret.as_deref().is_some_and(|s| !s.is_empty())
    }
}

pub async fn user(db: &SqlitePool, id: i64) -> sqlx::Result<Option<User>> {
    sqlx::query_as("SELECT * FROM users WHERE id = ?").bind(id).fetch_optional(db).await
}

pub async fn user_by_email(db: &SqlitePool, email: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as("SELECT * FROM users WHERE email = ?").bind(email).fetch_optional(db).await
}

/// Laravel's default bcrypt cost.
const BCRYPT_COST: u32 = 10;

pub async fn hash_password(password: &str) -> Result<String> {
    let password = password.to_string();
    Ok(tokio::task::spawn_blocking(move || bcrypt::hash(password, BCRYPT_COST)).await??)
}

/// Verifies against Laravel's `$2y$` hashes as well as our own.
pub async fn verify_password(password: &str, hash: &str) -> bool {
    let (password, hash) = (password.to_string(), hash.to_string());
    tokio::task::spawn_blocking(move || bcrypt::verify(password, &hash).unwrap_or(false))
        .await
        .unwrap_or(false)
}

/// Verified against when the email is unknown, so response times do not reveal which accounts exist.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| bcrypt::hash("unknown user", BCRYPT_COST).unwrap());

/// Like Fortify: five attempts per minute for each email and client address.
#[derive(Clone, Default)]
pub struct LoginLimiter(Arc<Mutex<HashMap<String, Vec<Instant>>>>);

const ATTEMPTS: usize = 5;
const WINDOW: Duration = Duration::from_secs(60);

impl LoginLimiter {
    /// Seconds until another attempt is allowed, or None to proceed.
    fn retry_after(&self, key: &str) -> Option<u64> {
        let mut attempts = self.0.lock().unwrap();
        let now = Instant::now();
        attempts.retain(|_, times| {
            times.retain(|t| now.duration_since(*t) < WINDOW);
            !times.is_empty()
        });
        let times = attempts.get(key)?;
        (times.len() >= ATTEMPTS).then(|| (WINDOW - now.duration_since(times[0])).as_secs().max(1))
    }

    fn hit(&self, key: &str) {
        self.0.lock().unwrap().entry(key.to_string()).or_default().push(Instant::now());
    }

    fn clear(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

/// The logged-in user; unauthenticated requests are redirected to the login page.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> std::result::Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state).await.map_err(IntoResponse::into_response)?;
        let id: Option<i64> = session.get(USER_ID).await.ok().flatten();
        let user = match id {
            Some(id) => user(&state.db, id).await.map_err(|e| Error::from(e).into_response())?,
            None => None,
        };
        match user {
            Some(user) => Ok(CurrentUser(user)),
            None => {
                if parts.method == Method::GET {
                    let _ = session.insert(INTENDED, parts.uri.path().to_string()).await;
                }
                Err(Redirect::to("/login").into_response())
            }
        }
    }
}

/// Rejects cross-site form posts. Browsers always send Origin or Sec-Fetch-Site on
/// unsafe requests, which works for multipart uploads where a token in the body could
/// only be checked after receiving gigabytes.
pub async fn same_origin(request: Request, next: Next) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS) || is_same_origin(&request) {
        next.run(request).await
    } else {
        (StatusCode::FORBIDDEN, "Cross-site request refused").into_response()
    }
}

fn is_same_origin(request: &Request) -> bool {
    let headers = request.headers();
    let header = |name| headers.get(name).and_then(|v: &header::HeaderValue| v.to_str().ok());
    if let Some(origin) = header(header::ORIGIN) {
        let host = header(header::HOST).unwrap_or_default();
        return origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")) == Some(host);
    }
    match header(axum::http::HeaderName::from_static("sec-fetch-site")) {
        Some(site) => site == "same-origin" || site == "none",
        None => true,
    }
}

/// The peer address, absent when the router runs without connection info (as in tests).
pub struct ClientIp(pub String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> std::result::Result<Self, Self::Rejection> {
        let ip = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(addr)| addr.ip().to_canonical().to_string());
        Ok(ClientIp(ip.unwrap_or_default()))
    }
}

async fn finish_login(session: &Session, user_id: i64, remember: bool) -> Result<Redirect> {
    // A fresh id prevents fixation of a session id planted before login.
    session.cycle_id().await?;
    session.remove::<i64>(PENDING_USER_ID).await?;
    session.remove::<bool>(PENDING_REMEMBER).await?;
    session.insert(USER_ID, user_id).await?;
    if remember {
        session.set_expiry(Some(Expiry::OnInactivity(session::REMEMBERED)));
    }
    let intended: Option<String> = session.remove(INTENDED).await?;
    Ok(Redirect::to(intended.as_deref().filter(|p| p.starts_with('/')).unwrap_or("/dashboard")))
}

pub async fn home(session: Session) -> Result<Redirect> {
    let logged_in: Option<i64> = session.get(USER_ID).await?;
    Ok(Redirect::to(if logged_in.is_some() { "/dashboard" } else { "/login" }))
}

pub async fn login_form(session: Session) -> Result<Response> {
    if session.get::<i64>(USER_ID).await?.is_some() {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    templates::render(templates::Login { email: String::new(), error: None })
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    remember: Option<String>,
}

pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    session: Session,
    Form(form): Form<LoginForm>,
) -> Result<Response> {
    let email = form.email.trim().to_string();
    let key = format!("{}|{ip}", email.to_lowercase());
    let failed = |error: String| {
        templates::render_status(StatusCode::UNPROCESSABLE_ENTITY, templates::Login { email: email.clone(), error: Some(error) })
    };

    if let Some(seconds) = state.login_limiter.retry_after(&key) {
        return failed(format!("Too many login attempts. Please try again in {seconds} seconds."));
    }
    let user = user_by_email(&state.db, &email).await?;
    let dummy = tokio::task::spawn_blocking(|| DUMMY_HASH.as_str()).await?;
    let hash = user.as_ref().map_or(dummy, |u| u.password.as_str());
    let valid = verify_password(&form.password, hash).await;
    let Some(user) = user.filter(|_| valid) else {
        state.login_limiter.hit(&key);
        return failed(BAD_CREDENTIALS.to_string());
    };
    state.login_limiter.clear(&key);

    let remember = form.remember.is_some();
    if user.has_two_factor() {
        session.insert(PENDING_USER_ID, user.id).await?;
        session.insert(PENDING_REMEMBER, remember).await?;
        return Ok(Redirect::to("/two-factor-challenge").into_response());
    }
    Ok(finish_login(&session, user.id, remember).await?.into_response())
}

pub async fn logout(session: Session) -> Result<Redirect> {
    session.flush().await?;
    Ok(Redirect::to("/"))
}

pub async fn challenge_form(session: Session) -> Result<Response> {
    if session.get::<i64>(PENDING_USER_ID).await?.is_none() {
        return Ok(Redirect::to("/login").into_response());
    }
    templates::render(templates::TwoFactorChallenge { error: None })
}

#[derive(Deserialize)]
pub struct ChallengeForm {
    code: Option<String>,
    recovery_code: Option<String>,
}

pub async fn challenge(State(state): State<AppState>, session: Session, Form(form): Form<ChallengeForm>) -> Result<Response> {
    let Some(id) = session.get::<i64>(PENDING_USER_ID).await? else {
        return Ok(Redirect::to("/login").into_response());
    };
    let user = user(&state.db, id).await?.context("pending user disappeared")?;
    let code = form.code.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let recovery = form.recovery_code.as_deref().map(str::trim).filter(|c| !c.is_empty());

    let valid = match (code, recovery) {
        (Some(code), _) => user
            .two_factor_secret
            .as_deref()
            .and_then(|s| twofactor::verify_step(s, &user.email, code))
            .is_some_and(|step| state.used_steps.accept(user.id, step)),
        (None, Some(recovery)) => twofactor::consume_recovery_code(&state.db, &user, recovery).await?,
        (None, None) => false,
    };
    if !valid {
        return templates::render_status(StatusCode::UNPROCESSABLE_ENTITY, templates::TwoFactorChallenge { error: Some(BAD_CODE) });
    }
    let remember = session.get::<bool>(PENDING_REMEMBER).await?.unwrap_or(false);
    Ok(finish_login(&session, user.id, remember).await?.into_response())
}

pub async fn flash(session: &Session, message: impl Into<String>) -> Result<()> {
    session.insert(templates::FLASH, Flash { message: message.into() }).await?;
    Ok(())
}
