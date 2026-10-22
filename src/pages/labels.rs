use std::sync::LazyLock;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use regex::Regex;
use serde::Deserialize;
use tower_sessions::Session;

use crate::auth::{self, CurrentUser, User};
use crate::models::{self, Label};
use crate::templates::{self, Errors, Nav};
use crate::web::{Error, Result};
use crate::{AppState, label};

/// Shown instead of the stored FTP password, which is never sent to the browser.
const UNCHANGED: &str = "****************";
static EXTENSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9]+$").unwrap());

#[derive(Template)]
#[template(path = "labels.html")]
pub struct LabelsPage {
    pub nav: Nav,
    pub labels: Vec<Label>,
}

#[derive(Default, Deserialize)]
pub struct LabelFields {
    pub name: String,
    pub printer_type: String,
    #[serde(default)]
    pub print_command: String,
    #[serde(default)]
    pub ftp_hostname: String,
    #[serde(default)]
    pub ftp_username: String,
    #[serde(default)]
    pub ftp_password: String,
    pub file_extension: String,
    pub template: String,
}

#[derive(Template)]
#[template(path = "label_form.html")]
pub struct LabelForm {
    pub nav: Nav,
    pub id: Option<i64>,
    pub fields: LabelFields,
    pub errors: Errors,
    pub message: Option<String>,
}

pub async fn index(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    templates::render(LabelsPage { nav: Nav::new(&session, &user, "labels").await?, labels: models::labels(&state.db).await? })
}

async fn form(session: &Session, user: &User, id: Option<i64>, fields: LabelFields, errors: Errors, message: Option<String>) -> Result<Response> {
    let status = if errors.is_empty() { StatusCode::OK } else { StatusCode::UNPROCESSABLE_ENTITY };
    templates::render_status(status, LabelForm { nav: Nav::new(session, user, "labels").await?, id, fields, errors, message })
}

pub async fn new(CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let fields = LabelFields {
        printer_type: "command".into(),
        print_command: "/usr/bin/lpr -o raw $file 2>&1".into(),
        file_extension: "jscript".into(),
        ..LabelFields::default()
    };
    form(&session, &user, None, fields, Errors::default(), None).await
}

pub async fn edit(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Response> {
    let l = models::label(&state.db, Some(id)).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    let fields = LabelFields {
        name: l.name,
        printer_type: l.printer_type,
        print_command: l.print_command.unwrap_or_default(),
        ftp_hostname: l.ftp_hostname.unwrap_or_default(),
        ftp_username: l.ftp_username.unwrap_or_default(),
        ftp_password: if l.ftp_password.unwrap_or_default().is_empty() { String::new() } else { UNCHANGED.into() },
        file_extension: l.file_extension,
        template: l.template,
    };
    form(&session, &user, Some(id), fields, Errors::default(), None).await
}

fn validate(fields: &mut LabelFields) -> Errors {
    fields.name = fields.name.trim().to_string();
    fields.template = fields.template.replace('\r', "");
    let mut errors = Errors::default();
    let required = |v: &str| !v.trim().is_empty() && v.len() <= 255;
    if !required(&fields.name) {
        errors.add("name", "The name field is required and may not be longer than 255 characters.");
    }
    match fields.printer_type.as_str() {
        "ftp" => {
            for (field, value) in [("ftp_hostname", &fields.ftp_hostname), ("ftp_username", &fields.ftp_username), ("ftp_password", &fields.ftp_password)] {
                if !required(value) {
                    errors.add(field, "This field is required when printing over FTP.");
                }
            }
        }
        "command" => {
            if !required(&fields.print_command) {
                errors.add("print_command", "The print command is required.");
            }
        }
        _ => errors.add("printer_type", "Choose FTP or a custom command."),
    }
    if !EXTENSION.is_match(&fields.file_extension) || fields.file_extension.len() > 32 {
        errors.add("file_extension", "The file extension may only contain letters and digits.");
    }
    if fields.template.trim().is_empty() {
        errors.add("template", "The template field is required.");
    }
    errors
}

pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Form(fields): Form<LabelFields>) -> Result<Response> {
    store(state, user, session, None, fields).await
}

pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Path(id): Path<i64>,
    Form(fields): Form<LabelFields>,
) -> Result<Response> {
    models::label(&state.db, Some(id)).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    store(state, user, session, Some(id), fields).await
}

async fn store(state: AppState, user: User, session: Session, id: Option<i64>, mut fields: LabelFields) -> Result<Response> {
    let mut errors = validate(&mut fields);
    if sqlx::query_scalar::<_, i64>("SELECT id FROM labels WHERE name = ?")
        .bind(&fields.name)
        .fetch_optional(&state.db)
        .await?
        .is_some_and(|other| Some(other) != id)
    {
        errors.add("name", "The name has already been taken.");
    }
    if !errors.is_empty() {
        return form(&session, &user, id, fields, errors, None).await;
    }

    let now = state.clock.timestamp();
    let password = (fields.ftp_password != UNCHANGED).then_some(&fields.ftp_password);
    match id {
        Some(id) => {
            sqlx::query(
                "UPDATE labels SET name = ?, printer_type = ?, print_command = ?, ftp_hostname = ?, ftp_username = ?, \
                 ftp_password = coalesce(?, ftp_password), file_extension = ?, template = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&fields.name)
            .bind(&fields.printer_type)
            .bind(&fields.print_command)
            .bind(&fields.ftp_hostname)
            .bind(&fields.ftp_username)
            .bind(password)
            .bind(&fields.file_extension)
            .bind(&fields.template)
            .bind(&now)
            .bind(id)
            .execute(&state.db)
            .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO labels (name, printer_type, print_command, ftp_hostname, ftp_username, ftp_password, file_extension, template, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&fields.name)
            .bind(&fields.printer_type)
            .bind(&fields.print_command)
            .bind(&fields.ftp_hostname)
            .bind(&fields.ftp_username)
            .bind(password)
            .bind(&fields.file_extension)
            .bind(&fields.template)
            .bind(&now)
            .bind(&now)
            .execute(&state.db)
            .await?;
        }
    }
    Ok(Redirect::to("/labels").into_response())
}

#[derive(Deserialize)]
pub struct TestTarget {
    id: Option<i64>,
}

/// Prints the form's current, possibly unsaved, settings with sample values.
pub async fn test_print(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    axum::extract::Query(target): axum::extract::Query<TestTarget>,
    Form(mut fields): Form<LabelFields>,
) -> Result<Response> {
    let errors = validate(&mut fields);
    if !errors.is_empty() {
        return form(&session, &user, target.id, fields, errors, None).await;
    }
    let mut password = fields.ftp_password.clone();
    if password == UNCHANGED {
        let stored = models::label(&state.db, target.id).await?;
        password = stored.and_then(|l| l.ftp_password).unwrap_or_default();
    }
    let printer = label::Label {
        printer: match fields.printer_type.as_str() {
            "ftp" => label::Printer::Ftp { hostname: fields.ftp_hostname.clone(), username: fields.ftp_username.clone(), password },
            _ => label::Printer::Command { command: fields.print_command.clone() },
        },
        file_extension: fields.file_extension.clone(),
        template: fields.template.clone(),
    };
    let serial = "1000000012345678";
    let contents = label::render(&printer.template, "11:22:33:44:55:66", serial, "000 (0)");
    let result = tokio::task::spawn_blocking(move || label::print(&printer, serial, &contents)).await?;
    let message = Some(result.err().unwrap_or_else(|| "Printed test label".into()));
    form(&session, &user, target.id, fields, Errors::default(), message).await
}

pub async fn delete(State(state): State<AppState>, _: CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    sqlx::query("DELETE FROM labels WHERE id = ?").bind(id).execute(&state.db).await?;
    auth::flash(&session, "Label deleted.").await?;
    Ok(Redirect::to("/labels"))
}
