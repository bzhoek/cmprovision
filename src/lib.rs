pub mod config;
pub mod db;
pub mod dnsmasq;
pub mod eeprom;
pub mod firmware;
pub mod images;
pub mod label;
pub mod models;
pub mod provision;
pub mod script;
pub mod switch;
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
}

impl AppState {
    /// Must run inside a Tokio runtime, which drives the image hashing worker.
    pub fn new(config: Config, db: SqlitePool, clock: Clock) -> Self {
        let hasher = images::Hasher::spawn(db.clone(), config.uploads_dir());
        AppState { config, db, clock, hasher }
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
