use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

pub async fn connect(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .foreign_keys(true)
        // The PHP app opened the database in rollback-journal mode; keep it so
        // both can be run against the same file during the transition.
        .journal_mode(SqliteJournalMode::Delete);

    SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .with_context(|| format!("opening database {}", path.display()))
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    let fresh = !table_exists(pool, "users").await?;

    sqlx::migrate!().run(pool).await.context("running migrations")?;

    // Laravel only seeded on first install; reseeding an adopted database
    // would resurrect scripts and labels the user deliberately deleted.
    if fresh {
        sqlx::raw_sql(include_str!("seed.sql"))
            .execute(pool)
            .await
            .context("seeding database")?;
    }
    Ok(())
}

async fn table_exists(pool: &SqlitePool, name: &str) -> Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
        .bind(name)
        .fetch_one(pool)
        .await?;
    Ok(count > 0)
}
