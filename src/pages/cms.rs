use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::auth::{self, CurrentUser};
use crate::models::{self, CmDetail, Project};
use crate::templates::{self, Nav};
use crate::web::{Error, Result};
use crate::AppState;

pub struct ProjectOption {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

#[derive(Template)]
#[template(path = "cms.html")]
pub struct CmsPage {
    pub nav: Nav,
    pub projects: Vec<ProjectOption>,
    pub selected: i64,
    pub cms: Vec<CmDetail>,
}

#[derive(Template)]
#[template(path = "cm.html")]
pub struct CmPage {
    pub nav: Nav,
    pub serial: String,
    pub rows: Vec<(&'static str, String)>,
    pub outputs: Vec<(&'static str, String)>,
}

fn detail_rows(cm: &CmDetail, project: Option<&Project>) -> Vec<(&'static str, String)> {
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    vec![
        ("MAC", cm.mac.clone()),
        ("Model", s(&cm.model)),
        ("Memory", cm.memory_in_gb.map(|m| format!("{m} GB")).unwrap_or_default()),
        ("Storage", cm.storage.map(|b| format!("{} GB", (b as f64 / 1e9).round())).unwrap_or_default()),
        ("CSD", s(&cm.csd)),
        ("CID", s(&cm.cid)),
        ("EEPROM firmware", s(&cm.firmware)),
        ("Project", project.map(|p| p.name.clone()).unwrap_or_default()),
        ("Provisioning board", s(&cm.provisioning_board)),
        ("Image written", s(&cm.image_filename)),
        ("Image SHA256", s(&cm.image_sha256)),
        ("Script return code", cm.script_return_code.map(|c| c.to_string()).unwrap_or_default()),
        ("Temperature at start", s(&cm.temp1)),
        ("Temperature at end", s(&cm.temp2)),
        ("First seen", s(&cm.created_at)),
        ("Provisioning started", s(&cm.provisioning_started_at)),
        ("Provisioning completed", s(&cm.provisioning_complete_at)),
        ("Duration", duration(cm).unwrap_or_default()),
    ]
}

#[derive(Deserialize)]
pub struct Filter {
    project: Option<i64>,
}

/// Without a filter the active project is shown, like the Livewire page; 0 means all projects.
async fn selected_project(state: &AppState, filter: &Filter) -> Result<Option<i64>> {
    Ok(match filter.project {
        Some(0) => None,
        Some(id) => Some(id),
        None => models::active_project_id(&state.db).await?,
    })
}

pub async fn index(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Query(filter): Query<Filter>) -> Result<Response> {
    let selected = selected_project(&state, &filter).await?;
    let counts: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT projects.id, projects.name, count(cms.id) FROM projects LEFT JOIN cms ON cms.project_id = projects.id \
         GROUP BY projects.id ORDER BY projects.name",
    )
    .fetch_all(&state.db)
    .await?;
    templates::render(CmsPage {
        nav: Nav::new(&session, &user, "cms").await?,
        projects: counts.into_iter().map(|(id, name, count)| ProjectOption { id, name, count }).collect(),
        selected: selected.unwrap_or(0),
        cms: models::cms(&state.db, selected).await?,
    })
}

fn duration(cm: &CmDetail) -> Option<String> {
    let parse = |t: &Option<String>| chrono::NaiveDateTime::parse_from_str(t.as_deref()?, "%Y-%m-%d %H:%M:%S").ok();
    let seconds = (parse(&cm.provisioning_complete_at)? - parse(&cm.provisioning_started_at)?).num_seconds();
    Some(format!("{}m {}s", seconds / 60, seconds % 60))
}

pub async fn show(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Response> {
    let cm = models::cm(&state.db, id).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    let project = match cm.project_id {
        Some(id) => models::project(&state.db, id).await?,
        None => None,
    };
    let outputs = [("Pre-install script output", &cm.pre_script_output), ("Post-install script output", &cm.post_script_output)]
        .into_iter()
        .filter_map(|(title, output)| Some((title, output.clone()?)))
        .collect();
    templates::render(CmPage {
        nav: Nav::new(&session, &user, "cms").await?,
        serial: cm.serial.clone(),
        rows: detail_rows(&cm, project.as_ref()),
        outputs,
    })
}

pub async fn delete(State(state): State<AppState>, _: CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    sqlx::query("DELETE FROM cms WHERE id = ?").bind(id).execute(&state.db).await?;
    auth::flash(&session, "Cm deleted.").await?;
    Ok(Redirect::to("/cms"))
}

const CSV_COLUMNS: &[&str] = &[
    "id", "serial", "mac", "model", "memory_in_gb", "storage", "cid", "csd", "firmware", "image_filename", "image_sha256",
    "pre_script_output", "post_script_output", "script_return_code", "temp1", "temp2", "project_id", "provisioning_board",
    "provisioning_started_at", "provisioning_complete_at", "created_at", "updated_at",
];

fn csv_row(cm: &CmDetail) -> Vec<String> {
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let n = |v: Option<i64>| v.map(|v| v.to_string()).unwrap_or_default();
    vec![
        cm.id.to_string(), cm.serial.clone(), cm.mac.clone(), s(&cm.model), n(cm.memory_in_gb), n(cm.storage), s(&cm.cid),
        s(&cm.csd), s(&cm.firmware), s(&cm.image_filename), s(&cm.image_sha256), s(&cm.pre_script_output),
        s(&cm.post_script_output), n(cm.script_return_code), s(&cm.temp1), s(&cm.temp2), n(cm.project_id),
        s(&cm.provisioning_board), s(&cm.provisioning_started_at), s(&cm.provisioning_complete_at), s(&cm.created_at),
        s(&cm.updated_at),
    ]
}

/// Quoting as PHP's fputcsv() did, so spreadsheets built on the old export keep working.
pub fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r', '\t', ' ', '\\']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

pub fn csv(cms: &[CmDetail]) -> String {
    if cms.is_empty() {
        return String::new();
    }
    let line = |fields: Vec<String>| fields.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",") + "\n";
    let mut out = line(CSV_COLUMNS.iter().map(|c| c.to_string()).collect());
    for cm in cms {
        out.push_str(&line(csv_row(cm)));
    }
    out
}

pub async fn export(State(state): State<AppState>, _: CurrentUser, Query(filter): Query<Filter>) -> Result<Response> {
    let cms = models::cms(&state.db, selected_project(&state, &filter).await?).await?;
    let filename = format!("export-cm-{}.csv", state.clock.now().format("%Y%m%d"));
    Ok((
        [(header::CONTENT_TYPE, "text/csv; charset=UTF-8".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename={filename}"))],
        csv(&cms),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_like_fputcsv() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("two words"), "\"two words\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(csv_field(""), "");
    }
}
