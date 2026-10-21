//! The token-authenticated JSON API under /api, with the response shapes Laravel's
//! Eloquent serialization produced, so existing integrations keep working.

use anyhow::Context;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Map, Value, json};
use sqlx::sqlite::SqliteRow;
use sqlx::{Column, Row, TypeInfo, ValueRef};

use crate::images::UploadError;
use crate::models;
use crate::pages::projects::{self, ProjectFields};
use crate::templates::Errors;
use crate::tokens::{ApiUser, json_error};
use crate::{AppState, firmware, images};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cms", get(cms))
        .route("/projects", get(projects_index))
        .route("/projects/{id}", get(project).patch(update_project))
        .route("/projects/{id}/cms", get(project_cms))
        .route("/images", get(images_index).post(upload_image))
        .route("/images/{id}", get(image).delete(delete_image))
        .route("/firmware", get(firmware_index))
        .route("/scripts", get(scripts))
        .route("/labels", get(labels))
}

pub enum ApiError {
    Response(Box<Response>),
    Internal(anyhow::Error),
}

impl ApiError {
    fn respond(response: impl IntoResponse) -> Self {
        ApiError::Response(Box::new(response.into_response()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Response(response) => *response,
            ApiError::Internal(err) => {
                tracing::error!("{err:#}");
                json_error(StatusCode::INTERNAL_SERVER_ERROR, "Server Error")
            }
        }
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(err: E) -> Self {
        ApiError::Internal(err.into())
    }
}

impl From<crate::web::Error> for ApiError {
    fn from(err: crate::web::Error) -> Self {
        match err {
            crate::web::Error::Status(status) => ApiError::respond(json_error(status, status.canonical_reason().unwrap_or_default())),
            crate::web::Error::Internal(err) => ApiError::Internal(err),
        }
    }
}

type Result<T> = std::result::Result<T, ApiError>;

/// Columns Eloquent cast or hid, per table.
struct Model {
    table: &'static str,
    class: &'static str,
    booleans: &'static [&'static str],
    datetimes: &'static [&'static str],
    hidden: &'static [&'static str],
}

const TIMESTAMPS: &[&str] = &["created_at", "updated_at"];
const CM: Model = Model {
    table: "cms",
    class: "Cm",
    booleans: &[],
    datetimes: &["created_at", "updated_at", "provisioning_started_at", "provisioning_complete_at"],
    hidden: &[],
};
const PROJECT: Model = Model { table: "projects", class: "Project", booleans: &["verify"], datetimes: TIMESTAMPS, hidden: &[] };
const IMAGE: Model = Model { table: "images", class: "Image", booleans: &[], datetimes: TIMESTAMPS, hidden: &[] };
const SCRIPT: Model = Model { table: "scripts", class: "Script", booleans: &["bg"], datetimes: TIMESTAMPS, hidden: &[] };
const LABEL: Model = Model { table: "labels", class: "Label", booleans: &[], datetimes: TIMESTAMPS, hidden: &["ftp_password"] };

/// `2024-01-02 03:04:05` as Laravel 8 serialized dates: ISO 8601 with microseconds in UTC.
fn iso8601(stored: &str) -> Value {
    match chrono::NaiveDateTime::parse_from_str(stored, "%Y-%m-%d %H:%M:%S") {
        Ok(t) => json!(t.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()),
        Err(_) => json!(stored),
    }
}

impl Model {
    fn to_json(&self, row: &SqliteRow) -> Value {
        let mut object = Map::new();
        for column in row.columns() {
            let name = column.name();
            if self.hidden.contains(&name) {
                continue;
            }
            let raw = row.try_get_raw(column.ordinal()).unwrap();
            let value = if raw.is_null() {
                Value::Null
            } else {
                match raw.type_info().name() {
                    "INTEGER" if self.booleans.contains(&name) => json!(row.get::<i64, _>(column.ordinal()) != 0),
                    "INTEGER" => json!(row.get::<i64, _>(column.ordinal())),
                    "REAL" => json!(row.get::<f64, _>(column.ordinal())),
                    _ if self.datetimes.contains(&name) => iso8601(&row.get::<String, _>(column.ordinal())),
                    _ => json!(row.get::<String, _>(column.ordinal())),
                }
            };
            object.insert(name.to_string(), value);
        }
        Value::Object(object)
    }

    async fn all(&self, state: &AppState, order: &str) -> Result<Json<Value>> {
        let sql = format!("SELECT * FROM {} ORDER BY {order}", self.table);
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql)).fetch_all(&state.db).await?;
        Ok(Json(Value::Array(rows.iter().map(|r| self.to_json(r)).collect())))
    }

    async fn find(&self, state: &AppState, id: i64) -> Result<Value> {
        let sql = format!("SELECT * FROM {} WHERE id = ?", self.table);
        let row = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id).fetch_optional(&state.db).await?;
        row.map(|r| self.to_json(&r)).ok_or_else(|| self.not_found(id))
    }

    fn not_found(&self, id: i64) -> ApiError {
        ApiError::respond(json_error(StatusCode::NOT_FOUND, &format!(r"No query results for model [App\Models\{}] {id}", self.class)))
    }
}

/// Laravel's validation failure response.
fn invalid(errors: &Errors) -> ApiError {
    let errors: Map<String, Value> = errors.0.iter().map(|(field, message)| (field.to_string(), json!([message]))).collect();
    ApiError::respond((StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "message": "The given data was invalid.", "errors": errors }))))
}

fn require(user: &ApiUser, ability: &str) -> Result<()> {
    user.require(ability).map_err(ApiError::respond)
}

async fn cms(State(state): State<AppState>, _: ApiUser) -> Result<Json<Value>> {
    CM.all(&state, "id").await
}

async fn project_cms(State(state): State<AppState>, _: ApiUser, Path(id): Path<i64>) -> Result<Json<Value>> {
    let rows = sqlx::query("SELECT * FROM cms WHERE project_id = ? ORDER BY id").bind(id).fetch_all(&state.db).await?;
    Ok(Json(Value::Array(rows.iter().map(|r| CM.to_json(r)).collect())))
}

async fn projects_index(State(state): State<AppState>, _: ApiUser) -> Result<Json<Value>> {
    PROJECT.all(&state, "name").await
}

async fn project(State(state): State<AppState>, _: ApiUser, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(PROJECT.find(&state, id).await?))
}

async fn images_index(State(state): State<AppState>, _: ApiUser) -> Result<Json<Value>> {
    IMAGE.all(&state, "filename, id").await
}

async fn image(State(state): State<AppState>, _: ApiUser, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(IMAGE.find(&state, id).await?))
}

async fn scripts(State(state): State<AppState>, _: ApiUser) -> Result<Json<Value>> {
    SCRIPT.all(&state, "name").await
}

async fn labels(State(state): State<AppState>, _: ApiUser) -> Result<Json<Value>> {
    LABEL.all(&state, "name").await
}

async fn firmware_index(State(state): State<AppState>, _: ApiUser) -> Json<Value> {
    let list = firmware::list(&state.config.firmware_dir());
    Json(Value::Array(list.into_iter().map(|f| json!({ "name": f.name, "channel": f.channel, "path": f.path })).collect()))
}

async fn upload_image(State(state): State<AppState>, user: ApiUser, mut multipart: Multipart) -> Result<Response> {
    require(&user, "create")?;
    match images::store(&state, &mut multipart).await {
        Ok(image) => Ok((StatusCode::CREATED, Json(IMAGE.find(&state, image.id).await?)).into_response()),
        Err(err @ (UploadError::Unsupported | UploadError::Missing)) => {
            let mut errors = Errors::default();
            errors.add("image", err.to_string());
            Err(invalid(&errors))
        }
        Err(UploadError::Internal(err)) => Err(err.into()),
    }
}

async fn delete_image(State(state): State<AppState>, user: ApiUser, Path(id): Path<i64>) -> Result<Response> {
    require(&user, "delete")?;
    let image = models::image(&state.db, Some(id)).await?.ok_or_else(|| IMAGE.not_found(id))?;
    images::delete(&state, &image).await?;
    Ok(StatusCode::OK.into_response())
}

/// Accepts what PHP's loose typing did: numbers or numeric strings, and booleans as 1/"1"/"true".
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "1" } else { "" }.to_string()),
        _ => None,
    }
}

fn as_id(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn as_bool(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64() != Some(0),
        Value::String(s) => !matches!(s.as_str(), "" | "0" | "false"),
        _ => false,
    }
}

/// Updates the fields Eloquent allowed to be mass-assigned; others are ignored as they were.
async fn update_project(State(state): State<AppState>, user: ApiUser, Path(id): Path<i64>, Json(patch): Json<Map<String, Value>>) -> Result<Json<Value>> {
    require(&user, "update")?;
    let project = models::project(&state.db, id).await?.ok_or_else(|| PROJECT.not_found(id))?;
    let mut fields = ProjectFields {
        name: project.name,
        storage: project.storage,
        image_id: project.image_id,
        verify: project.verify,
        firmware: project.eeprom_firmware.unwrap_or_default(),
        eeprom_settings: project.eeprom_settings.unwrap_or_default(),
        label_moment: project.label_moment,
        label_id: project.label_id,
        scripts: models::project_script_ids(&state.db, id).await?,
        active: false,
    };
    let mut errors = Errors::default();
    for (key, value) in &patch {
        match key.as_str() {
            "name" => fields.name = as_text(value).unwrap_or_default().trim().to_string(),
            "storage" => fields.storage = as_text(value).unwrap_or_default().trim().to_string(),
            "image_id" => fields.image_id = as_id(value),
            "label_id" => fields.label_id = as_id(value),
            "label_moment" => fields.label_moment = as_text(value).unwrap_or_default(),
            "eeprom_firmware" => fields.firmware = as_text(value).unwrap_or_default(),
            "eeprom_settings" => fields.eeprom_settings = as_text(value).unwrap_or_default().replace('\r', ""),
            "verify" => fields.verify = as_bool(value),
            "device" if as_text(value).as_deref() != Some("cm4") => errors.add("device", "The selected device is invalid."),
            _ => {}
        }
    }
    for (field, message) in projects::check(&state, Some(id), &fields).await?.0 {
        errors.add(field, message);
    }
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    projects::save(&state, Some(id), &fields).await?;

    // PHP left the staged EEPROM image of the active project stale after an API update.
    if models::active_project_id(&state.db).await? == Some(id) {
        let project = models::project(&state.db, id).await?.context("reading back project")?;
        if let Some(warning) = projects::activate(&state, &project).await? {
            tracing::warn!("re-activating project {id}: {warning}");
        }
    }
    Ok(Json(PROJECT.find(&state, id).await?))
}
