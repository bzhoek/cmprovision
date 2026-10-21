//! Rows of the Laravel-era schema. Timestamps stay `Y-m-d H:i:s` strings in UTC,
//! the format Eloquent wrote, so both generations of the app read the same data.

use sqlx::{FromRow, SqliteExecutor, SqlitePool};

use crate::label;

#[derive(Clone, Debug, FromRow)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub device: String,
    pub storage: String,
    pub image_id: Option<i64>,
    pub label_id: Option<i64>,
    pub label_moment: String,
    pub eeprom_firmware: Option<String>,
    pub eeprom_settings: Option<String>,
    pub verify: bool,
}

#[derive(Clone, Debug, FromRow)]
pub struct Image {
    pub id: i64,
    pub filename: String,
    pub filename_on_server: String,
    pub filename_extension: String,
    pub sha256: String,
    pub uncompressed_sha256: Option<String>,
    pub uncompressed_size: Option<i64>,
    pub created_at: Option<String>,
}

#[derive(Clone, Debug, FromRow)]
pub struct Script {
    pub id: i64,
    pub name: String,
    pub script_type: String,
    pub bg: bool,
    pub priority: i64,
    pub script: String,
}

#[derive(Clone, Debug, FromRow)]
pub struct Label {
    pub id: i64,
    pub name: String,
    pub printer_type: String,
    pub print_command: Option<String>,
    pub ftp_hostname: Option<String>,
    pub ftp_username: Option<String>,
    pub ftp_password: Option<String>,
    pub file_extension: String,
    pub template: String,
}

impl Label {
    pub fn printer(&self) -> label::Label {
        let printer = match self.printer_type.as_str() {
            "ftp" => label::Printer::Ftp {
                hostname: self.ftp_hostname.clone().unwrap_or_default(),
                username: self.ftp_username.clone().unwrap_or_default(),
                password: self.ftp_password.clone().unwrap_or_default(),
            },
            _ => label::Printer::Command { command: self.print_command.clone().unwrap_or_default() },
        };
        label::Label { printer, file_extension: self.file_extension.clone(), template: self.template.clone() }
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct Cm {
    pub id: i64,
    pub serial: String,
    pub mac: String,
    pub project_id: Option<i64>,
    pub provisioning_board: Option<String>,
}

/// Every column, for the detail page and CSV export.
#[derive(Clone, Debug, FromRow)]
pub struct CmDetail {
    pub id: i64,
    pub serial: String,
    pub mac: String,
    pub model: Option<String>,
    pub memory_in_gb: Option<i64>,
    pub storage: Option<i64>,
    pub cid: Option<String>,
    pub csd: Option<String>,
    pub firmware: Option<String>,
    pub image_filename: Option<String>,
    pub image_sha256: Option<String>,
    pub pre_script_output: Option<String>,
    pub post_script_output: Option<String>,
    pub script_return_code: Option<i64>,
    pub temp1: Option<String>,
    pub temp2: Option<String>,
    pub project_id: Option<i64>,
    pub provisioning_board: Option<String>,
    pub provisioning_started_at: Option<String>,
    pub provisioning_complete_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, FromRow)]
pub struct Host {
    pub id: i64,
    pub ip: String,
    pub mac: String,
    pub hostname: Option<String>,
}

pub async fn setting(db: impl SqliteExecutor<'_>, key: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(r#"SELECT "value" FROM settings WHERE "key" = ?"#).bind(key).fetch_optional(db).await
}

pub async fn set_setting(db: impl SqliteExecutor<'_>, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query(r#"INSERT INTO settings ("key", "value") VALUES (?, ?) ON CONFLICT ("key") DO UPDATE SET "value" = excluded."value""#)
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_setting(db: impl SqliteExecutor<'_>, key: &str) -> sqlx::Result<()> {
    sqlx::query(r#"DELETE FROM settings WHERE "key" = ?"#).bind(key).execute(db).await?;
    Ok(())
}

/// Parsed leniently like PHP's intval(), so "0" or junk means none.
pub async fn active_project_id(db: &SqlitePool) -> sqlx::Result<Option<i64>> {
    Ok(setting(db, "active_project").await?.and_then(|v| v.trim().parse::<i64>().ok()).filter(|&id| id != 0))
}

pub async fn active_project(db: &SqlitePool) -> sqlx::Result<Option<Project>> {
    match active_project_id(db).await? {
        Some(id) => project(db, id).await,
        None => Ok(None),
    }
}

pub async fn projects(db: &SqlitePool) -> sqlx::Result<Vec<Project>> {
    sqlx::query_as("SELECT * FROM projects ORDER BY name").fetch_all(db).await
}

pub async fn images(db: &SqlitePool) -> sqlx::Result<Vec<Image>> {
    sqlx::query_as("SELECT * FROM images ORDER BY filename, id").fetch_all(db).await
}

pub async fn scripts(db: &SqlitePool) -> sqlx::Result<Vec<Script>> {
    sqlx::query_as("SELECT * FROM scripts ORDER BY name").fetch_all(db).await
}

pub async fn script(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Script>> {
    sqlx::query_as("SELECT * FROM scripts WHERE id = ?").bind(id).fetch_optional(db).await
}

pub async fn labels(db: &SqlitePool) -> sqlx::Result<Vec<Label>> {
    sqlx::query_as("SELECT * FROM labels ORDER BY name").fetch_all(db).await
}

pub async fn hosts(db: &SqlitePool) -> sqlx::Result<Vec<Host>> {
    sqlx::query_as("SELECT * FROM hosts ORDER BY ip").fetch_all(db).await
}

pub async fn cms(db: &SqlitePool, project_id: Option<i64>) -> sqlx::Result<Vec<CmDetail>> {
    match project_id {
        Some(id) => sqlx::query_as("SELECT * FROM cms WHERE project_id = ? ORDER BY id").bind(id).fetch_all(db).await,
        None => sqlx::query_as("SELECT * FROM cms ORDER BY id").fetch_all(db).await,
    }
}

pub async fn cm(db: &SqlitePool, id: i64) -> sqlx::Result<Option<CmDetail>> {
    sqlx::query_as("SELECT * FROM cms WHERE id = ?").bind(id).fetch_optional(db).await
}

pub async fn project_script_ids(db: &SqlitePool, project_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT script_id FROM project_script WHERE project_id = ?").bind(project_id).fetch_all(db).await
}

pub async fn project(db: impl SqliteExecutor<'_>, id: i64) -> sqlx::Result<Option<Project>> {
    sqlx::query_as("SELECT * FROM projects WHERE id = ?").bind(id).fetch_optional(db).await
}

pub async fn image(db: impl SqliteExecutor<'_>, id: Option<i64>) -> sqlx::Result<Option<Image>> {
    let Some(id) = id else { return Ok(None) };
    sqlx::query_as("SELECT * FROM images WHERE id = ?").bind(id).fetch_optional(db).await
}

pub async fn label(db: impl SqliteExecutor<'_>, id: Option<i64>) -> sqlx::Result<Option<Label>> {
    let Some(id) = id else { return Ok(None) };
    sqlx::query_as("SELECT * FROM labels WHERE id = ?").bind(id).fetch_optional(db).await
}

pub async fn project_scripts(db: impl SqliteExecutor<'_>, project_id: i64, script_type: &str) -> sqlx::Result<Vec<Script>> {
    sqlx::query_as(
        "SELECT scripts.* FROM scripts JOIN project_script ON project_script.script_id = scripts.id \
         WHERE project_script.project_id = ? AND scripts.script_type = ? \
         ORDER BY scripts.priority, scripts.id",
    )
    .bind(project_id)
    .bind(script_type)
    .fetch_all(db)
    .await
}

pub async fn cm_by_serial(db: impl SqliteExecutor<'_>, serial: &str) -> sqlx::Result<Option<Cm>> {
    sqlx::query_as("SELECT id, serial, mac, project_id, provisioning_board FROM cms WHERE serial = ?")
        .bind(serial)
        .fetch_optional(db)
        .await
}

pub async fn log(
    db: impl SqliteExecutor<'_>,
    now: &str,
    cm: &str,
    board: Option<&str>,
    ip: Option<&str>,
    loglevel: &str,
    msg: &str,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO cmlogs (cm, board, ip, loglevel, msg, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(cm)
        .bind(board)
        .bind(ip)
        .bind(loglevel)
        .bind(msg)
        .bind(now)
        .bind(now)
        .execute(db)
        .await?;
    Ok(())
}
