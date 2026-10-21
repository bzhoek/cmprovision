pub mod config;
pub mod db;
pub mod dnsmasq;
pub mod eeprom;
pub mod firmware;
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
