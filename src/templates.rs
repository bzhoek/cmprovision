//! Server-rendered pages. Templates live in templates/ and are compiled in by askama.

use std::collections::HashMap;

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::auth::User;
use crate::web::Result;

pub const FLASH: &str = "flash";

#[derive(Serialize, Deserialize)]
pub struct Flash {
    pub message: String,
}

pub fn render(template: impl Template) -> Result<Response> {
    render_status(StatusCode::OK, template)
}

pub fn render_status(status: StatusCode, template: impl Template) -> Result<Response> {
    Ok((status, Html(template.render()?)).into_response())
}

/// The navigation bar and the one-shot message shown after a redirect.
pub struct Nav {
    pub user_name: String,
    pub active: &'static str,
    pub flash: Option<String>,
}

impl Nav {
    pub async fn new(session: &Session, user: &User, active: &'static str) -> Result<Self> {
        let flash: Option<Flash> = session.remove(FLASH).await?;
        Ok(Nav { user_name: user.name.clone(), active, flash: flash.map(|f| f.message) })
    }

    pub fn links(&self) -> &'static [(&'static str, &'static str, &'static str)] {
        &[
            ("dashboard", "/dashboard", "Dashboard"),
            ("cms", "/cms", "CMs"),
            ("images", "/images", "Images"),
            ("projects", "/projects", "Projects"),
            ("scripts", "/scripts", "Scripts"),
            ("labels", "/labels", "Labels"),
            ("firmware", "/firmware", "Firmware"),
            ("settings", "/settings", "Settings"),
        ]
    }
}

/// Validation messages by form field.
#[derive(Default)]
pub struct Errors(pub HashMap<&'static str, String>);

impl Errors {
    pub fn add(&mut self, field: &'static str, message: impl Into<String>) {
        self.0.entry(field).or_insert_with(|| message.into());
    }

    pub fn get(&self, field: &str) -> Option<&str> {
        self.0.get(field).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct Login {
    pub email: String,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "two_factor_challenge.html")]
pub struct TwoFactorChallenge {
    pub error: Option<&'static str>,
}

pub struct LogRow {
    pub board: String,
    pub cm: String,
    pub time: String,
    pub lines: Vec<String>,
    pub error: bool,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct Dashboard {
    pub nav: Nav,
    pub log: Vec<LogRow>,
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct Profile {
    pub nav: Nav,
    pub name: String,
    pub email: String,
    pub profile_errors: Errors,
    pub password_errors: Errors,
    pub two_factor_error: Option<String>,
    pub two_factor_enabled: bool,
    pub pending_qr: Option<String>,
    pub pending_secret: Option<String>,
    pub recovery_codes: Vec<String>,
}

pub struct TokenRow {
    pub id: i64,
    pub name: String,
    pub abilities: Vec<String>,
    pub last_used_at: Option<String>,
}

impl TokenRow {
    pub fn has(&self, permission: &&str) -> bool {
        self.abilities.iter().any(|a| a == permission)
    }
}

#[derive(Template)]
#[template(path = "api_tokens.html")]
pub struct ApiTokens {
    pub nav: Nav,
    pub tokens: Vec<TokenRow>,
    pub permissions: &'static [&'static str],
    pub defaults: &'static [&'static str],
    pub new_token: Option<String>,
    pub name: String,
    pub errors: Errors,
}
