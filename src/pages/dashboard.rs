use axum::extract::State;
use axum::response::Response;
use sqlx::FromRow;
use tower_sessions::Session;

use crate::auth::CurrentUser;
use crate::templates::{self, Dashboard, LogRow, Nav};
use crate::web::Result;
use crate::AppState;

#[derive(FromRow)]
struct Cmlog {
    board: Option<String>,
    cm: String,
    created_at: Option<String>,
    loglevel: String,
    msg: String,
}

pub async fn show(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let rows: Vec<Cmlog> = sqlx::query_as("SELECT board, cm, created_at, loglevel, msg FROM cmlogs ORDER BY id DESC LIMIT 100")
        .fetch_all(&state.db)
        .await?;
    let log = rows
        .into_iter()
        .map(|row| LogRow {
            board: row.board.unwrap_or_default(),
            cm: row.cm,
            time: row.created_at.as_deref().and_then(|t| t.split_once(' ')).map(|(_, time)| time.to_string()).unwrap_or_default(),
            lines: row.msg.lines().map(str::to_string).collect(),
            error: row.loglevel == "error",
        })
        .collect();
    templates::render(Dashboard { nav: Nav::new(&session, &user, "dashboard").await?, log })
}
