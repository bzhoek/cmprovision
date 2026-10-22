use std::io::{Seek, Write};

use anyhow::{Context, bail};
use askama::Template;
use axum::extract::State;
use axum::response::{Redirect, Response};
use futures_util::StreamExt;
use tower_sessions::Session;

use crate::auth::{self, CurrentUser};
use crate::firmware::{self, Firmware};
use crate::templates::{self, Nav};
use crate::web::Result;
use crate::AppState;

#[derive(Template)]
#[template(path = "firmware.html")]
pub struct FirmwarePage {
    pub nav: Nav,
    pub firmware: Vec<Firmware>,
}

pub async fn index(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    templates::render(FirmwarePage { nav: Nav::new(&session, &user, "firmware").await?, firmware: firmware::list(&state.config.firmware_dir()) })
}

/// Streams the zipball to a temporary file; it holds every bootloader ever released.
async fn download() -> anyhow::Result<std::fs::File> {
    let response = reqwest::Client::builder()
        // GitHub's API refuses requests without a User-Agent.
        .user_agent(concat!("cmprovision/", env!("CARGO_PKG_VERSION")))
        .build()?
        .get(firmware::ZIPBALL_URL)
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("Expected HTTP response code 200, but received {}", response.status());
    }
    let mut file = tempfile::tempfile()?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        file.write_all(&chunk?)?;
    }
    file.rewind()?;
    Ok(file)
}

pub async fn update(State(state): State<AppState>, _: CurrentUser, session: Session) -> Result<Redirect> {
    let result = async {
        let file = download().await.context("downloading firmware")?;
        let dest = state.config.firmware_dir();
        tokio::task::spawn_blocking(move || firmware::extract_zipball(file, &dest)).await?
    }
    .await;
    let message = match result {
        Ok(()) => "Firmware updated".to_string(),
        Err(err) => format!("Error: {err:#}"),
    };
    auth::flash(&session, message).await?;
    Ok(Redirect::to("/firmware"))
}
