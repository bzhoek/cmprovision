pub mod config;
pub mod db;
pub mod dnsmasq;
pub mod eeprom;
pub mod firmware;
pub mod label;
pub mod switch;
pub mod web;

use config::Config;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: SqlitePool,
}
