//! API tokens in Laravel Sanctum's format, so tokens issued by the PHP app keep working:
//! clients send `<id>|<secret>`, the database holds the SHA256 of the secret.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rand::distr::{Alphanumeric, SampleString};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};

use crate::{AppState, Clock};

pub const PERMISSIONS: &[&str] = &["create", "read", "update", "delete"];
pub const DEFAULT_PERMISSIONS: &[&str] = &["read"];
const TOKENABLE_TYPE: &str = r"App\Models\User";

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct Token {
    pub id: i64,
    pub name: String,
    #[serde(skip)]
    pub token: String,
    pub abilities: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: Option<String>,
}

impl Token {
    pub fn abilities(&self) -> Vec<String> {
        self.abilities.as_deref().and_then(|a| serde_json::from_str(a).ok()).unwrap_or_default()
    }

    pub fn can(&self, ability: &str) -> bool {
        self.abilities().iter().any(|a| a == "*" || a == ability)
    }
}

fn digest(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Returns the plain-text token, which is shown to the user once and never stored.
pub async fn create(db: &SqlitePool, clock: &Clock, user_id: i64, name: &str, permissions: &[&str]) -> sqlx::Result<String> {
    let secret = Alphanumeric.sample_string(&mut rand::rng(), 40);
    let now = clock.timestamp();
    let id = sqlx::query(
        "INSERT INTO personal_access_tokens (tokenable_type, tokenable_id, name, token, abilities, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(TOKENABLE_TYPE)
    .bind(user_id)
    .bind(name)
    .bind(digest(&secret))
    .bind(serde_json::to_string(permissions).unwrap())
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await?
    .last_insert_rowid();
    Ok(format!("{id}|{secret}"))
}

pub async fn list(db: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Token>> {
    sqlx::query_as(
        "SELECT id, name, token, abilities, last_used_at, created_at FROM personal_access_tokens \
         WHERE tokenable_type = ? AND tokenable_id = ? ORDER BY name, id",
    )
    .bind(TOKENABLE_TYPE)
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn update_permissions(db: &SqlitePool, clock: &Clock, user_id: i64, id: i64, permissions: &[&str]) -> sqlx::Result<()> {
    sqlx::query("UPDATE personal_access_tokens SET abilities = ?, updated_at = ? WHERE id = ? AND tokenable_type = ? AND tokenable_id = ?")
        .bind(serde_json::to_string(permissions).unwrap())
        .bind(clock.timestamp())
        .bind(id)
        .bind(TOKENABLE_TYPE)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, user_id: i64, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM personal_access_tokens WHERE id = ? AND tokenable_type = ? AND tokenable_id = ?")
        .bind(id)
        .bind(TOKENABLE_TYPE)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

#[derive(FromRow)]
struct Owned {
    #[sqlx(flatten)]
    token: Token,
    tokenable_id: i64,
}

const SELECT_OWNED: &str = "SELECT id, name, token, abilities, last_used_at, created_at, tokenable_id FROM personal_access_tokens";

/// Sanctum accepts both `<id>|<secret>` and, for older tokens, the bare secret.
async fn find(db: &SqlitePool, presented: &str) -> sqlx::Result<Option<Owned>> {
    match presented.split_once('|') {
        Some((id, secret)) => {
            let Ok(id) = id.parse::<i64>() else { return Ok(None) };
            sqlx::query_as(owned_where(" WHERE id = ? AND token = ? AND tokenable_type = ?"))
                .bind(id)
                .bind(digest(secret))
                .bind(TOKENABLE_TYPE)
                .fetch_optional(db)
                .await
        }
        None => {
            sqlx::query_as(owned_where(" WHERE token = ? AND tokenable_type = ?"))
                .bind(digest(presented))
                .bind(TOKENABLE_TYPE)
                .fetch_optional(db)
                .await
        }
    }
}

fn owned_where(condition: &'static str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(format!("{SELECT_OWNED}{condition}"))
}

/// A request authenticated with a bearer token.
pub struct ApiUser {
    pub user_id: i64,
    pub token: Token,
}

pub struct Forbidden(String);

impl IntoResponse for Forbidden {
    fn into_response(self) -> Response {
        json_error(StatusCode::FORBIDDEN, &format!("API user lacks '{}' permission", self.0))
    }
}

impl ApiUser {
    pub fn require(&self, ability: &str) -> Result<(), Forbidden> {
        if self.token.can(ability) { Ok(()) } else { Err(Forbidden(ability.to_string())) }
    }
}

pub fn json_error(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "message": message }))).into_response()
}

impl FromRequestParts<AppState> for ApiUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let unauthenticated = || json_error(StatusCode::UNAUTHORIZED, "Unauthenticated.");
        let presented = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .ok_or_else(unauthenticated)?;
        let Owned { token, tokenable_id: user_id } = find(&state.db, presented)
            .await
            .map_err(|e| crate::web::Error::from(e).into_response())?
            .ok_or_else(unauthenticated)?;
        let _ = sqlx::query("UPDATE personal_access_tokens SET last_used_at = ? WHERE id = ?")
            .bind(state.clock.timestamp())
            .bind(token.id)
            .execute(&state.db)
            .await;
        Ok(ApiUser { user_id, token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_grants_everything() {
        let token = Token { id: 1, name: "t".into(), token: String::new(), abilities: Some(r#"["*"]"#.into()), last_used_at: None, created_at: None };
        assert!(token.can("delete"));
    }
}
