use askama::Template;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{Redirect, Response};
use tower_sessions::Session;

use crate::auth::{self, CurrentUser};
use crate::images::{self, UploadError};
use crate::models;
use crate::pages::gigabytes;
use crate::templates::{self, Nav};
use crate::web::{Error, Result};
use crate::{AppState, system};

pub struct ImageRow {
    pub id: i64,
    pub filename: String,
    pub compressed: String,
    pub uncompressed: String,
    pub sha256: String,
    pub uncompressed_sha256: String,
}

#[derive(Template)]
#[template(path = "images.html")]
pub struct ImagesPage {
    pub nav: Nav,
    pub images: Vec<ImageRow>,
    pub free_space: String,
}

pub async fn index(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    let uploads = state.config.uploads_dir();
    let images = models::images(&state.db)
        .await?
        .into_iter()
        .map(|i| {
            let size = std::fs::metadata(images::path(&uploads, &i)).map(|m| m.len()).ok();
            ImageRow {
                id: i.id,
                compressed: size.map_or("missing".into(), gigabytes),
                uncompressed: i.uncompressed_size.map_or("unknown".into(), |s| gigabytes(s as u64)),
                sha256: if i.sha256.is_empty() { "…still computing hash…".into() } else { i.sha256 },
                uncompressed_sha256: i.uncompressed_sha256.unwrap_or_default(),
                filename: i.filename,
            }
        })
        .collect();
    let probe = if uploads.exists() { uploads } else { state.config.data_dir.clone() };
    templates::render(ImagesPage {
        nav: Nav::new(&session, &user, "images").await?,
        images,
        free_space: system::free_space(&probe).map_or("unknown".into(), gigabytes),
    })
}

/// The browser posts with a progress bar via XMLHttpRequest, or as a plain form without JavaScript.
pub async fn upload(State(state): State<AppState>, _: CurrentUser, session: Session, mut multipart: Multipart) -> Result<Redirect> {
    match images::store(&state, &mut multipart).await {
        Ok(_) => auth::flash(&session, "Image added.").await?,
        Err(err @ (UploadError::Unsupported | UploadError::Missing)) => auth::flash(&session, err.to_string()).await?,
        Err(UploadError::Internal(err)) => return Err(err.into()),
    }
    Ok(Redirect::to("/images"))
}

pub async fn delete(State(state): State<AppState>, _: CurrentUser, session: Session, Path(id): Path<i64>) -> Result<Redirect> {
    let image = models::image(&state.db, Some(id)).await?.ok_or(Error::Status(StatusCode::NOT_FOUND))?;
    images::delete(&state, &image).await?;
    auth::flash(&session, "Image deleted.").await?;
    Ok(Redirect::to("/images"))
}
