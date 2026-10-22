//! Session storage for tower-sessions in the application's SQLite database.

use async_trait::async_trait;
use sqlx::SqlitePool;
use time::OffsetDateTime;
use tower_sessions::cookie::SameSite;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{Error, Result};
use tower_sessions::{Expiry, SessionManagerLayer, SessionStore};

pub const COOKIE: &str = "cmprovision_session";
/// Laravel's SESSION_LIFETIME of 120 minutes of inactivity.
pub const LIFETIME: time::Duration = time::Duration::minutes(120);
pub const REMEMBERED: time::Duration = time::Duration::days(30);

#[derive(Clone, Debug)]
pub struct SqliteStore {
    db: SqlitePool,
}

impl SqliteStore {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

fn backend(err: impl std::fmt::Display) -> Error {
    Error::Backend(err.to_string())
}

#[async_trait]
impl SessionStore for SqliteStore {
    async fn create(&self, record: &mut Record) -> Result<()> {
        sqlx::query("DELETE FROM web_sessions WHERE expires_at < ?")
            .bind(OffsetDateTime::now_utc().unix_timestamp())
            .execute(&self.db)
            .await
            .map_err(backend)?;
        let data = serde_json::to_string(&record.data).map_err(|e| Error::Encode(e.to_string()))?;
        loop {
            let inserted = sqlx::query("INSERT INTO web_sessions (id, data, expires_at) VALUES (?, ?, ?) ON CONFLICT DO NOTHING")
                .bind(record.id.to_string())
                .bind(&data)
                .bind(record.expiry_date.unix_timestamp())
                .execute(&self.db)
                .await
                .map_err(backend)?;
            if inserted.rows_affected() == 1 {
                return Ok(());
            }
            record.id = Id::default();
        }
    }

    async fn save(&self, record: &Record) -> Result<()> {
        let data = serde_json::to_string(&record.data).map_err(|e| Error::Encode(e.to_string()))?;
        sqlx::query(
            "INSERT INTO web_sessions (id, data, expires_at) VALUES (?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET data = excluded.data, expires_at = excluded.expires_at",
        )
        .bind(record.id.to_string())
        .bind(data)
        .bind(record.expiry_date.unix_timestamp())
        .execute(&self.db)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn load(&self, id: &Id) -> Result<Option<Record>> {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT data, expires_at FROM web_sessions WHERE id = ? AND expires_at >= ?")
                .bind(id.to_string())
                .bind(OffsetDateTime::now_utc().unix_timestamp())
                .fetch_optional(&self.db)
                .await
                .map_err(backend)?;
        let Some((data, expires_at)) = row else {
            return Ok(None);
        };
        Ok(Some(Record {
            id: *id,
            data: serde_json::from_str(&data).map_err(|e| Error::Decode(e.to_string()))?,
            expiry_date: OffsetDateTime::from_unix_timestamp(expires_at).map_err(|e| Error::Decode(e.to_string()))?,
        }))
    }

    async fn delete(&self, id: &Id) -> Result<()> {
        sqlx::query("DELETE FROM web_sessions WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.db)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

pub fn layer(db: SqlitePool) -> SessionManagerLayer<SqliteStore> {
    // Served over plain HTTP on an isolated provisioning network, so the cookie cannot be Secure.
    SessionManagerLayer::new(SqliteStore::new(db))
        .with_name(COOKIE)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(false)
        .with_expiry(Expiry::OnInactivity(LIFETIME))
}
