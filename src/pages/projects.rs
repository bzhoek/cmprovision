use std::sync::LazyLock;

use anyhow::Context;
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::Form;
use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tower_sessions::Session;

use crate::auth::{self, CurrentUser, User};
use crate::firmware::{self, Firmware};
use crate::models::{self, Image, Label, Project, Script};
use crate::pages::checked;
use crate::templates::{self, Errors, Nav};
use crate::web::{Error, Result};
use crate::{AppState, eeprom};

static STORAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/dev/[A-Za-z0-9_./-]+$").unwrap());
pub const LABEL_MOMENTS: &[(&str, &str)] =
    &[("never", "Never"), ("preinstall", "Before provisioning"), ("postinstall", "After provisioning completed successfully")];

pub struct ProjectRow {
    pub id: i64,
    pub name: String,
    pub image: String,
    pub active: bool,
}

#[derive(Template)]
#[template(path = "projects.html")]
pub struct ProjectsPage {
    pub nav: Nav,
    pub projects: Vec<ProjectRow>,
}

/// What the form shows, either from the database or as submitted.
#[derive(Default)]
pub struct ProjectFields {
    pub name: String,
    pub storage: String,
    pub image_id: Option<i64>,
    pub verify: bool,
    pub firmware: String,
    pub eeprom_settings: String,
    pub label_moment: String,
    pub label_id: Option<i64>,
    pub scripts: Vec<i64>,
    pub active: bool,
}

pub struct ImageOption {
    pub id: i64,
    pub label: String,
}

#[derive(Template)]
#[template(path = "project_form.html")]
pub struct ProjectForm {
    pub nav: Nav,
    pub id: Option<i64>,
    pub fields: ProjectFields,
    pub errors: Errors,
    pub images: Vec<ImageOption>,
    pub labels: Vec<Label>,
    pub scripts: Vec<Script>,
    pub channels: Vec<(String, Vec<Firmware>)>,
    pub moments: &'static [(&'static str, &'static str)],
    pub firmware_defaults: String,
}

impl ProjectForm {
    pub fn selected_script(&self, id: &i64) -> bool {
        self.fields.scripts.contains(id)
    }

    pub fn selected_image(&self, id: &i64) -> bool {
        self.fields.image_id == Some(*id)
    }

    pub fn selected_label(&self, id: &i64) -> bool {
        self.fields.label_id == Some(*id)
    }
}

pub async fn index(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let active = models::active_project_id(&state.db).await?;
    let rows: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT projects.id, projects.name, images.filename FROM projects LEFT JOIN images ON images.id = projects.image_id ORDER BY projects.name")
            .fetch_all(&state.db)
            .await?;
    let projects = rows
        .into_iter()
        .map(|(id, name, image)| ProjectRow { id, name, image: image.unwrap_or_default(), active: Some(id) == active })
        .collect();
    templates::render(ProjectsPage { nav: Nav::new(&session, &user, "projects").await?, projects })
}

fn image_option(image: Image) -> ImageOption {
    let added = image
        .created_at
        .as_deref()
        .and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").ok())
        .map(|t| format!(" (added {})", t.format("%d-%b-%Y")))
        .unwrap_or_default();
    ImageOption { id: image.id, label: format!("{}{added}", image.filename) }
}

fn channels(state: &AppState) -> Vec<(String, Vec<Firmware>)> {
    let mut channels: Vec<(String, Vec<Firmware>)> = Vec::new();
    for fw in firmware::list(&state.config.firmware_dir()) {
        match channels.last_mut() {
            Some((channel, list)) if *channel == fw.channel => list.push(fw),
            _ => channels.push((fw.channel.clone(), vec![fw])),
        }
    }
    channels
}

/// The bootconf.txt a firmware image ships with, as the starting point for a project's settings.
pub fn firmware_defaults(state: &AppState, path: &str) -> String {
    firmware::resolve(&state.config.firmware_dir(), path)
        .and_then(|file| std::fs::read(file).ok())
        .and_then(|data| eeprom::settings(&data).ok().map(|s| String::from_utf8_lossy(s).into_owned()))
        .unwrap_or_default()
}

async fn form(state: &AppState, session: &Session, user: &User, id: Option<i64>, fields: ProjectFields, errors: Errors) -> Result<Response> {
    let status = if errors.is_empty() { StatusCode::OK } else { StatusCode::UNPROCESSABLE_ENTITY };
    let page = ProjectForm {
        nav: Nav::new(session, user, "projects").await?,
        id,
        firmware_defaults: firmware_defaults(state, &fields.firmware),
        fields,
        errors,
        images: models::images(&state.db).await?.into_iter().map(image_option).collect(),
        labels: models::labels(&state.db).await?,
        scripts: models::scripts(&state.db).await?,
        channels: channels(state),
        moments: LABEL_MOMENTS,
    };
    templates::render_status(status, page)
}

pub async fn new(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let latest_image: Option<i64> = sqlx::query_scalar("SELECT max(id) FROM images").fetch_one(&state.db).await?;
    let fields = ProjectFields {
        storage: "/dev/mmcblk0".into(),
        image_id: latest_image,
        label_moment: "never".into(),
        active: true,
        ..ProjectFields::default()
    };
    form(&state, &session, &user, None, fields, Errors::default()).await
}

pub async fn edit(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Response> {
    let p = models::project(&state.db, id).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    let fields = ProjectFields {
        active: models::active_project_id(&state.db).await? == Some(id),
        scripts: models::project_script_ids(&state.db, id).await?,
        name: p.name,
        storage: p.storage,
        image_id: p.image_id,
        verify: p.verify,
        firmware: p.eeprom_firmware.unwrap_or_default(),
        eeprom_settings: p.eeprom_settings.unwrap_or_default(),
        label_moment: p.label_moment,
        label_id: p.label_id,
    };
    form(&state, &session, &user, Some(id), fields, Errors::default()).await
}

#[derive(Deserialize)]
pub struct ProjectInput {
    name: String,
    storage: String,
    image_id: Option<String>,
    verify: Option<String>,
    firmware: Option<String>,
    eeprom_settings: Option<String>,
    label_moment: String,
    label_id: Option<String>,
    #[serde(default)]
    scripts: Vec<i64>,
    active: Option<String>,
}

fn optional_id(value: &Option<String>) -> Option<i64> {
    value.as_deref().and_then(|v| v.parse().ok())
}

async fn validate(state: &AppState, id: Option<i64>, input: ProjectInput) -> Result<(ProjectFields, Errors)> {
    let fields = ProjectFields {
        name: input.name.trim().to_string(),
        storage: input.storage.trim().to_string(),
        image_id: optional_id(&input.image_id),
        verify: checked(&input.verify),
        firmware: input.firmware.unwrap_or_default(),
        eeprom_settings: input.eeprom_settings.unwrap_or_default().replace('\r', ""),
        label_id: optional_id(&input.label_id).filter(|_| input.label_moment != "never"),
        label_moment: input.label_moment,
        scripts: input.scripts,
        active: checked(&input.active),
    };
    let errors = check(state, id, &fields).await?;
    Ok((fields, errors))
}

/// Shared with the API, which patches individual fields of an existing project.
pub async fn check(state: &AppState, id: Option<i64>, fields: &ProjectFields) -> Result<Errors> {
    let mut errors = Errors::default();
    if fields.name.is_empty() || fields.name.len() > 255 {
        errors.add("name", "The name field is required and may not be longer than 255 characters.");
    } else if sqlx::query_scalar::<_, i64>("SELECT id FROM projects WHERE name = ?")
        .bind(&fields.name)
        .fetch_optional(&state.db)
        .await?
        .is_some_and(|other| Some(other) != id)
    {
        errors.add("name", "The name has already been taken.");
    }
    // Interpolated into the provisioning shell script, so only plain device paths.
    if !STORAGE.is_match(&fields.storage) || fields.storage.len() > 255 {
        errors.add("storage", "Enter a device path such as /dev/mmcblk0.");
    }
    if fields.image_id.is_some() && models::image(&state.db, fields.image_id).await?.is_none() {
        errors.add("image_id", "The selected image no longer exists.");
    }
    if !fields.firmware.is_empty() && firmware::resolve(&state.config.firmware_dir(), &fields.firmware).is_none() {
        errors.add("firmware", "The selected firmware is not available.");
    }
    if fields.eeprom_settings.len() > eeprom::MAX_BOOTCONF_SIZE {
        errors.add("eeprom_settings", format!("EEPROM settings may not be longer than {} bytes.", eeprom::MAX_BOOTCONF_SIZE));
    }
    if !LABEL_MOMENTS.iter().any(|(m, _)| *m == fields.label_moment) {
        errors.add("label_moment", "Choose when to print a label.");
    } else if fields.label_moment != "never" && models::label(&state.db, fields.label_id).await?.is_none() {
        errors.add("label_id", "Choose a label to print.");
    }
    Ok(errors)
}

pub async fn save(state: &AppState, id: Option<i64>, fields: &ProjectFields) -> Result<i64> {
    let now = state.clock.timestamp();
    let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let mut tx = state.db.begin().await?;
    let id = match id {
        Some(id) => {
            sqlx::query(
                "UPDATE projects SET name = ?, storage = ?, image_id = ?, label_id = ?, label_moment = ?, eeprom_firmware = ?, \
                 eeprom_settings = ?, verify = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&fields.name)
            .bind(&fields.storage)
            .bind(fields.image_id)
            .bind(fields.label_id)
            .bind(&fields.label_moment)
            .bind(nonempty(&fields.firmware))
            .bind(nonempty(&fields.eeprom_settings))
            .bind(fields.verify)
            .bind(&now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            id
        }
        None => sqlx::query(
            "INSERT INTO projects (name, device, storage, image_id, label_id, label_moment, eeprom_firmware, eeprom_settings, verify, created_at, updated_at) \
             VALUES (?, 'cm4', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&fields.name)
        .bind(&fields.storage)
        .bind(fields.image_id)
        .bind(fields.label_id)
        .bind(&fields.label_moment)
        .bind(nonempty(&fields.firmware))
        .bind(nonempty(&fields.eeprom_settings))
        .bind(fields.verify)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid(),
    };
    sqlx::query("DELETE FROM project_script WHERE project_id = ?").bind(id).execute(&mut *tx).await?;
    for script in &fields.scripts {
        sqlx::query("INSERT INTO project_script (project_id, script_id) SELECT ?, id FROM scripts WHERE id = ?")
            .bind(id)
            .bind(script)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(id)
}

pub async fn create(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Form(input): Form<ProjectInput>) -> Result<Response> {
    store(state, user, session, None, input).await
}

pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Path(id): Path<i64>,
    Form(input): Form<ProjectInput>,
) -> Result<Response> {
    models::project(&state.db, id).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    store(state, user, session, Some(id), input).await
}

async fn store(state: AppState, user: User, session: Session, id: Option<i64>, input: ProjectInput) -> Result<Response> {
    let (fields, errors) = validate(&state, id, input).await?;
    if !errors.is_empty() {
        return form(&state, &session, &user, id, fields, errors).await;
    }
    let id = save(&state, id, &fields).await?;
    let project = models::project(&state.db, id).await?.context("reading back project")?;
    if fields.active
        && let Some(warning) = activate(&state, &project).await?
    {
        auth::flash(&session, warning).await?;
    }
    if project.verify
        && let Some(image) = models::image(&state.db, project.image_id).await?.filter(|i| i.uncompressed_sha256.is_none())
    {
        state.hasher.enqueue(image.id);
    }
    Ok(Redirect::to("/projects").into_response())
}

/// Makes the project the one new devices get, and stages its EEPROM image for download.
/// Returns a message for the user when the firmware could not be prepared.
pub async fn activate(state: &AppState, project: &Project) -> Result<Option<String>> {
    models::set_setting(&state.db, "active_project", &project.id.to_string()).await?;

    let firmware_dir = state.config.firmware_dir();
    let mut warning = None;
    let firmware = project.eeprom_firmware.as_deref().filter(|f| !f.is_empty()).and_then(|f| {
        let file = firmware::resolve(&firmware_dir, f);
        if file.is_none() {
            warning = Some(format!("EEPROM firmware '{f}' no longer available"));
        }
        file
    });

    // Left behind by a beta of the PHP version that signed the update itself.
    let _ = std::fs::remove_file(state.config.data_dir.join("scriptexecute/pieeprom.sig"));
    let uploads = state.config.uploads_dir();
    let staged = uploads.join("pieeprom.bin");

    let Some(file) = firmware else {
        let _ = std::fs::remove_file(&staged);
        return Ok(warning);
    };
    let mut data = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
    // Without custom settings the image keeps the bootconf.txt it was released with;
    // PHP wrote empty settings instead, silently resetting the bootloader configuration.
    if let Some(settings) = project.eeprom_settings.as_deref().filter(|s| !s.is_empty())
        && eeprom::set_settings(&mut data, settings.as_bytes()).is_err()
    {
        return Ok(Some("Error parsing EEPROM file for configuration".into()));
    }
    std::fs::create_dir_all(&uploads)?;
    // Written aside and renamed, so a device downloading right now never gets half a file.
    let partial = uploads.join("pieeprom.bin.partial");
    std::fs::write(&partial, &data)?;
    std::fs::rename(&partial, &staged)?;
    models::set_setting(&state.db, "active_eeprom_sha256", &hex::encode(Sha256::digest(&data))).await?;
    Ok(warning)
}

pub async fn set_active(State(state): State<AppState>, _: CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    let project = models::project(&state.db, id).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    let warning = activate(&state, &project).await?;
    auth::flash(&session, warning.unwrap_or_else(|| format!("{} is now the active project.", project.name))).await?;
    Ok(Redirect::to("/projects"))
}

pub async fn delete(State(state): State<AppState>, _: CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    if models::active_project_id(&state.db).await? == Some(id) {
        models::delete_setting(&state.db, "active_project").await?;
    }
    sqlx::query("DELETE FROM projects WHERE id = ?").bind(id).execute(&state.db).await?;
    auth::flash(&session, "Project deleted.").await?;
    Ok(Redirect::to("/projects"))
}

#[derive(Deserialize)]
pub struct FirmwareQuery {
    path: String,
}

/// Lets the project form offer the selected firmware's default settings.
pub async fn bootconf(State(state): State<AppState>, _: CurrentUser, Query(query): Query<FirmwareQuery>) -> Response {
    firmware_defaults(&state, &query.path).into_response()
}
