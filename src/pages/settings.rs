use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::sync::LazyLock;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use regex::Regex;
use serde::Deserialize;
use tower_sessions::Session;

use crate::auth::{self, CurrentUser, User};
use crate::dnsmasq::{self, StaticHost};
use crate::models::{self, Host};
use crate::templates::{self, Errors, Nav};
use crate::web::{Error, Result};
use crate::{AppState, system};

static MAC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9a-f]{2}:){5}[0-9a-f]{2}$").unwrap());

pub struct Service {
    pub name: &'static str,
    pub label: &'static str,
    pub running: bool,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsPage {
    pub nav: Nav,
    pub services: Vec<Service>,
    pub hosts: Vec<Host>,
}

#[derive(Template)]
#[template(path = "service_log.html")]
pub struct LogPage {
    pub nav: Nav,
    pub title: &'static str,
    pub log: String,
}

#[derive(Template)]
#[template(path = "static_ip_form.html")]
pub struct StaticIpForm {
    pub nav: Nav,
    pub ip: String,
    pub mac: String,
    pub detected: Vec<String>,
    pub errors: Errors,
}

pub async fn index(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let mut services = Vec::new();
    for &(name, label) in system::LOGGED {
        services.push(Service { name, label, running: system::is_active(name).await });
    }
    templates::render(SettingsPage { nav: Nav::new(&session, &user, "settings").await?, services, hosts: models::hosts(&state.db).await? })
}

pub async fn log(CurrentUser(user): CurrentUser, session: Session, Path(service): Path<String>) -> Result<Response> {
    let (name, label) = system::LOGGED.iter().find(|(name, _)| *name == service).ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    templates::render(LogPage { nav: Nav::new(&session, &user, "settings").await?, title: label, log: system::journal(name).await })
}

async fn static_ip_form(session: &Session, user: &User, ip: String, mac: String, detected: Vec<String>, errors: Errors) -> Result<Response> {
    let status = if errors.is_empty() { StatusCode::OK } else { StatusCode::UNPROCESSABLE_ENTITY };
    templates::render_status(status, StaticIpForm { nav: Nav::new(session, user, "settings").await?, ip, mac, detected, errors })
}

async fn detected_macs() -> Vec<String> {
    dnsmasq::macs_without_lease(&system::journal(system::DNSMASQ).await)
}

/// Prefilled with the next free address and the device that most recently failed to get a lease.
pub async fn new_static_ip(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let hosts = models::hosts(&state.db).await?;
    let used: HashSet<String> = hosts.iter().map(|h| h.ip.clone()).collect();
    let detected = detected_macs().await;
    let mac = detected.last().filter(|m| !hosts.iter().any(|h| &h.mac == *m)).cloned().unwrap_or_default();
    let ip = dnsmasq::first_available_ip(&used).unwrap_or_default();
    static_ip_form(&session, &user, ip, mac, detected, Errors::default()).await
}

#[derive(Deserialize)]
pub struct StaticIpInput {
    ip: String,
    mac: String,
}

pub async fn create_static_ip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(input): Form<StaticIpInput>,
) -> Result<Response> {
    let ip = input.ip.trim().to_string();
    let mac = input.mac.trim().to_lowercase();
    let hosts = models::hosts(&state.db).await?;
    let mut errors = Errors::default();
    if ip.parse::<Ipv4Addr>().is_err() {
        errors.add("ip", "The ip must be a valid IPv4 address.");
    } else if hosts.iter().any(|h| h.ip == ip) {
        errors.add("ip", "The ip has already been taken.");
    }
    if !MAC.is_match(&mac) {
        errors.add("mac", "The mac format is invalid.");
    } else if hosts.iter().any(|h| h.mac == mac) {
        errors.add("mac", "The mac has already been taken.");
    }
    if !errors.is_empty() {
        return static_ip_form(&session, &user, ip, mac, detected_macs().await, errors).await;
    }

    let now = state.clock.timestamp();
    sqlx::query("INSERT INTO hosts (ip, mac, created_at, updated_at) VALUES (?, ?, ?, ?)")
        .bind(&ip)
        .bind(&mac)
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await?;
    auth::flash(&session, apply_static_hosts(&state).await?).await?;
    Ok(Redirect::to("/settings").into_response())
}

/// The PHP version only rewrote dnsmasq.conf when adding, so deleted leases lingered.
pub async fn delete_static_ip(State(state): State<AppState>, _: CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    sqlx::query("DELETE FROM hosts WHERE id = ?").bind(id).execute(&state.db).await?;
    let message = apply_static_hosts(&state).await?;
    auth::flash(&session, format!("Static IP deleted. {message}")).await?;
    Ok(Redirect::to("/settings"))
}

async fn apply_static_hosts(state: &AppState) -> Result<String> {
    let conf = state.config.dnsmasq_conf();
    let old = match tokio::fs::read_to_string(&conf).await {
        Ok(old) if !old.trim().is_empty() => old,
        _ => return Ok(format!("Error reading existing {}", conf.display())),
    };
    let hosts: Vec<StaticHost> = models::hosts(&state.db)
        .await?
        .into_iter()
        .map(|h| StaticHost { ip: h.ip, mac: h.mac, hostname: h.hostname })
        .collect();
    if tokio::fs::write(&conf, dnsmasq::render_conf(&old, &hosts)).await.is_err() {
        return Ok(format!("Error writing to {}. Check file permissions.", conf.display()));
    }
    Ok(if system::restart_dnsmasq().await {
        "Restarted dnsmasq.".into()
    } else {
        "Unable to restart dnsmasq automatically, please do so manually.".into()
    })
}
