pub mod auth;
pub mod config;
pub mod dashboard;
pub mod db;
pub mod dnsmasq;
pub mod eeprom;
pub mod firmware;
pub mod images;
pub mod label;
pub mod models;
pub mod profile;
pub mod provision;
pub mod script;
pub mod session;
pub mod switch;
pub mod templates;
pub mod tokens;
pub mod twofactor;
pub mod web;

use chrono::{NaiveDateTime, Utc};
use config::Config;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: SqlitePool,
    pub clock: Clock,
    pub hasher: images::Hasher,
    pub login_limiter: auth::LoginLimiter,
    pub used_steps: twofactor::UsedSteps,
}

impl AppState {
    /// Must run inside a Tokio runtime, which drives the image hashing worker.
    pub fn new(config: Config, db: SqlitePool, clock: Clock) -> Self {
        let hasher = images::Hasher::spawn(db.clone(), config.uploads_dir());
        AppState {
            config,
            db,
            clock,
            hasher,
            login_limiter: auth::LoginLimiter::default(),
            used_steps: twofactor::UsedSteps::default(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Clock {
    System,
    Fixed(NaiveDateTime),
}

impl Clock {
    pub fn now(&self) -> NaiveDateTime {
        match self {
            Clock::System => Utc::now().naive_utc(),
            Clock::Fixed(t) => *t,
        }
    }

    /// In the format Eloquent stored timestamps in.
    pub fn timestamp(&self) -> String {
        self.now().format("%Y-%m-%d %H:%M:%S").to_string()
    }
}
