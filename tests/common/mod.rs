//! A logged-in browser against an in-process server with a fresh data directory.
#![allow(dead_code)]

use std::path::PathBuf;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use clap::Parser;
use cmprovision::config::Config;
use cmprovision::{AppState, Clock, db, web};
use tower::ServiceExt;

pub const EMAIL: &str = "admin@example.com";
pub const PASSWORD: &str = "correct horse";
const HOST: &str = "cmprovision.local";

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    config: Config,
}

pub struct App {
    dir: tempfile::TempDir,
    pub state: AppState,
    router: Router,
    pub user_id: i64,
}

pub struct Browser<'a> {
    app: &'a App,
    pub cookie: Option<String>,
}

pub struct Page {
    pub status: StatusCode,
    pub location: Option<String>,
    pub headers: axum::http::HeaderMap,
    pub body: String,
}

impl App {
    pub async fn new() -> Self {
        Self::with_clock(Clock::System).await
    }

    /// Frozen at the time the PHP golden files were captured, on a database without the
    /// seeded scripts and label, so ids line up with the capture.
    pub async fn golden() -> Self {
        let time = chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap().and_hms_opt(3, 4, 5).unwrap();
        let app = Self::with_clock(Clock::Fixed(time)).await;
        sqlx::raw_sql("DELETE FROM scripts; DELETE FROM labels; DELETE FROM sqlite_sequence WHERE name IN ('scripts', 'labels');")
            .execute(&app.state.db)
            .await
            .unwrap();
        app
    }

    pub async fn with_clock(clock: Clock) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("db.sqlite");
        let config = Args::parse_from(["test", "--data-dir", dir.path().to_str().unwrap(), "--database", database.to_str().unwrap()]).config;
        let pool = db::connect(&database).await.unwrap();
        db::migrate(&pool).await.unwrap();
        // Laravel's hash prefix, to prove existing passwords keep working.
        let hash = bcrypt::hash_with_result(PASSWORD, 4).unwrap().format_for_version(bcrypt::Version::TwoY);
        let user_id = sqlx::query("INSERT INTO users (name, email, password) VALUES ('Admin', ?, ?)")
            .bind(EMAIL)
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let state = AppState::new(config, pool, clock);
        App { dir, router: web::router(state.clone()), state, user_id }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    pub fn browser(&self) -> Browser<'_> {
        Browser { app: self, cookie: None }
    }

    pub async fn logged_in(&self) -> Browser<'_> {
        let mut browser = self.browser();
        browser.login().await;
        browser
    }

    pub async fn exec(&self, sql: &'static str) -> i64 {
        sqlx::query(sql).execute(&self.state.db).await.unwrap().last_insert_rowid()
    }
}

impl Browser<'_> {
    pub async fn send(&mut self, request: axum::http::request::Builder, body: Body) -> Page {
        let mut request = request.header(header::HOST, HOST);
        if let Some(cookie) = &self.cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let response = self.app.router.clone().oneshot(request.body(body).unwrap()).await.unwrap();
        if let Some(set) = response.headers().get(header::SET_COOKIE) {
            self.cookie = Some(set.to_str().unwrap().split(';').next().unwrap().to_string());
        }
        let status = response.status();
        let headers = response.headers().clone();
        let location = headers.get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
        let body = String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).into_owned();
        Page { status, location, headers, body }
    }

    pub async fn get(&mut self, path: &str) -> Page {
        self.send(Request::get(path), Body::empty()).await
    }

    pub async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Page {
        let body = form_urlencoded::Serializer::new(String::new()).extend_pairs(form).finish();
        let request = Request::post(path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ORIGIN, format!("http://{HOST}"));
        self.send(request, Body::from(body)).await
    }

    pub async fn upload(&mut self, path: &str, field: &str, filename: &str, contents: &[u8]) -> Page {
        let mut body = format!(
            "--b\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(contents);
        body.extend_from_slice(b"\r\n--b--\r\n");
        let request = Request::post(path)
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=b")
            .header(header::ORIGIN, format!("http://{HOST}"));
        self.send(request, Body::from(body)).await
    }

    pub async fn api(&mut self, method: &str, path: &str, token: &str, json: Option<&str>) -> Page {
        let mut request = Request::builder().method(method).uri(path).header(header::AUTHORIZATION, format!("Bearer {token}"));
        let body = match json {
            Some(json) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        self.send(request, body).await
    }

    pub async fn login(&mut self) -> Page {
        self.get("/login").await;
        self.post("/login", &[("email", EMAIL), ("password", PASSWORD)]).await
    }

    /// Follows the redirect and returns the page with its flash message.
    pub async fn follow(&mut self, page: Page) -> Page {
        match page.location {
            Some(location) => self.get(&location).await,
            None => page,
        }
    }
}
