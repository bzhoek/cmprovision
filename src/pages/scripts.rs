use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use tower_sessions::Session;

use crate::auth::{self, CurrentUser, User};
use crate::models::{self, Script};
use crate::pages::checked;
use crate::templates::{self, Errors, Nav};
use crate::web::{Error, Result};
use crate::AppState;

#[derive(Template)]
#[template(path = "scripts.html")]
pub struct ScriptsPage {
    pub nav: Nav,
    pub scripts: Vec<Script>,
}

pub struct ScriptFields {
    pub name: String,
    pub script_type: String,
    pub priority: String,
    pub bg: bool,
    pub script: String,
}

#[derive(Template)]
#[template(path = "script_form.html")]
pub struct ScriptForm {
    pub nav: Nav,
    pub id: Option<i64>,
    pub fields: ScriptFields,
    pub errors: Errors,
}

pub async fn index(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    templates::render(ScriptsPage { nav: Nav::new(&session, &user, "scripts").await?, scripts: models::scripts(&state.db).await? })
}

async fn form(session: &Session, user: &User, id: Option<i64>, fields: ScriptFields, errors: Errors) -> Result<Response> {
    let status = if errors.is_empty() { StatusCode::OK } else { StatusCode::UNPROCESSABLE_ENTITY };
    templates::render_status(status, ScriptForm { nav: Nav::new(session, user, "scripts").await?, id, fields, errors })
}

pub async fn new(CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let fields = ScriptFields {
        name: String::new(),
        script_type: "postinstall".into(),
        priority: "100".into(),
        bg: false,
        script: "#!/bin/sh\nset -e\n\n".into(),
    };
    form(&session, &user, None, fields, Errors::default()).await
}

pub async fn edit(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Response> {
    let s = models::script(&state.db, id).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    let fields = ScriptFields { name: s.name, script_type: s.script_type, priority: s.priority.to_string(), bg: s.bg, script: s.script };
    form(&session, &user, Some(id), fields, Errors::default()).await
}

#[derive(Deserialize)]
pub struct ScriptInput {
    name: String,
    script_type: String,
    priority: String,
    bg: Option<String>,
    script: String,
}

pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Form(input): Form<ScriptInput>) -> Result<Response> {
    store(state, user, session, None, input).await
}

pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Path(id): Path<i64>,
    Form(input): Form<ScriptInput>,
) -> Result<Response> {
    models::script(&state.db, id).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    store(state, user, session, Some(id), input).await
}

async fn store(state: AppState, user: User, session: Session, id: Option<i64>, input: ScriptInput) -> Result<Response> {
    let fields = ScriptFields {
        name: input.name.trim().to_string(),
        script_type: input.script_type,
        priority: input.priority.trim().to_string(),
        bg: checked(&input.bg),
        script: input.script.replace('\r', ""),
    };
    let mut errors = Errors::default();
    if fields.name.is_empty() || fields.name.len() > 255 {
        errors.add("name", "The name field is required and may not be longer than 255 characters.");
    } else if sqlx::query_scalar::<_, i64>("SELECT id FROM scripts WHERE name = ?")
        .bind(&fields.name)
        .fetch_optional(&state.db)
        .await?
        .is_some_and(|other| Some(other) != id)
    {
        errors.add("name", "The name has already been taken.");
    }
    if fields.script_type != "preinstall" && fields.script_type != "postinstall" {
        errors.add("script_type", "Choose preinstall or postinstall.");
    }
    let priority = fields.priority.parse::<i64>();
    if priority.is_err() {
        errors.add("priority", "The priority must be a whole number.");
    }
    // The heredoc that carries the script to the device ends at this line.
    if fields.script.trim().is_empty() {
        errors.add("script", "The script field is required.");
    } else if fields.script.lines().any(|l| l == "CMPROVISIONINGEOF") {
        errors.add("script", "The script may not contain a line reading CMPROVISIONINGEOF.");
    }
    if !errors.is_empty() {
        return form(&session, &user, id, fields, errors).await;
    }

    let now = state.clock.timestamp();
    let query = match id {
        Some(id) => sqlx::query("UPDATE scripts SET name = ?, script_type = ?, priority = ?, bg = ?, script = ?, updated_at = ? WHERE id = ?")
            .bind(&fields.name)
            .bind(&fields.script_type)
            .bind(priority.unwrap_or(100))
            .bind(fields.bg)
            .bind(&fields.script)
            .bind(&now)
            .bind(id),
        None => sqlx::query("INSERT INTO scripts (name, script_type, priority, bg, script, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&fields.name)
            .bind(&fields.script_type)
            .bind(priority.unwrap_or(100))
            .bind(fields.bg)
            .bind(&fields.script)
            .bind(&now)
            .bind(&now),
    };
    query.execute(&state.db).await?;
    Ok(Redirect::to("/scripts").into_response())
}

pub async fn delete(State(state): State<AppState>, _: CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    sqlx::query("DELETE FROM scripts WHERE id = ?").bind(id).execute(&state.db).await?;
    auth::flash(&session, "Script deleted.").await?;
    Ok(Redirect::to("/scripts"))
}
