//! Replays test_api of tests/Golden/CaptureTest.php and compares with the PHP responses.

mod common;

use axum::http::StatusCode;
use cmprovision::tokens;
use common::App;
use serde_json::{Value, json};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/api");
const NOW: &str = "2024-01-02 03:04:05";

/// The same rows the PHP capture created, then a provisioning request for the CM.
async fn scenario(app: &App) {
    for sql in [
        "INSERT INTO images (filename, filename_on_server, filename_extension, sha256, uncompressed_sha256, uncompressed_size, created_at, updated_at) VALUES ('raspios.img.gz', 'abcdefghijklmnopqrstuvwxyz0123456789ABCD.gz', 'gz', 'bb' || hex(zeroblob(31)), 'aa' || hex(zeroblob(31)), 1073741824, ?1, ?1)",
        "INSERT INTO labels (name, printer_type, ftp_hostname, ftp_username, ftp_password, file_extension, template, created_at, updated_at) VALUES ('Ftp', 'ftp', 'printer', 'print', 'secret', 'jscript', '$mac', ?1, ?1)",
        "INSERT INTO scripts (name, script_type, priority, bg, script, created_at, updated_at) VALUES ('pre-a', 'preinstall', 100, 0, '#!/bin/sh' || char(10) || 'echo ''pre-a'' \"$STORAGE\"' || char(10), ?1, ?1)",
        "INSERT INTO projects (name, device, storage, image_id, label_id, label_moment, verify, created_at, updated_at) VALUES ('Golden', 'cm4', '/dev/mmcblk0', 1, 1, 'never', 0, ?1, ?1)",
        "INSERT INTO project_script (project_id, script_id) VALUES (1, 1)",
        "INSERT INTO settings (\"key\", \"value\") VALUES ('active_project', '1')",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql)).bind(NOW).execute(&app.state.db).await.unwrap();
    }
    let query = "serial=10000000abcdef12&model=CM4&memorysize=3884000&storagesize=61071360&cid=15010038474e443352&csd=d00f00320f5903ff&temp=45.2";
    let page = app.browser().get(&format!("/scriptexecute?{query}")).await;
    assert_eq!(page.status, StatusCode::OK);
}

async fn token(app: &App, abilities: &[&str]) -> String {
    tokens::create(&app.state.db, &app.state.clock, app.user_id, "golden", abilities).await.unwrap()
}

fn golden(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/{name}.json")).unwrap()).unwrap()
}

async fn assert_golden(app: &App, token: &str, name: &str, method: &str, path: &str, body: Option<&str>) {
    let page = app.browser().api(method, path, token, body).await;
    let actual = json!({ "status": page.status.as_u16(), "body": serde_json::from_str::<Value>(&page.body).unwrap_or(Value::Null) });
    assert_eq!(actual, golden(name), "{name}: {method} {path}");
}

#[tokio::test]
async fn responses_match_php() {
    let app = App::golden().await;
    scenario(&app).await;
    let token = token(&app, &["read", "update"]).await;

    for (name, path) in [
        ("cms", "/api/cms"),
        ("project_cms", "/api/projects/1/cms"),
        ("projects", "/api/projects"),
        ("images", "/api/images"),
        ("image", "/api/images/1"),
        ("image_missing", "/api/images/999"),
        ("firmware", "/api/firmware"),
        ("scripts", "/api/scripts"),
        ("labels", "/api/labels"),
        ("project", "/api/projects/1"),
    ] {
        assert_golden(&app, &token, name, "GET", path, None).await;
    }
    assert_golden(&app, &token, "project_patch", "PATCH", "/api/projects/1", Some(r#"{"name": "Renamed", "verify": true}"#)).await;
    assert_golden(&app, &token, "image_delete_forbidden", "DELETE", "/api/images/1", None).await;
}

#[tokio::test]
async fn requires_a_valid_token() {
    let app = App::new().await;
    let valid = token(&app, &["read"]).await;
    let (id, _) = valid.split_once('|').unwrap();

    for presented in ["", "nonsense", &format!("{id}|wrong")] {
        let page = app.browser().api("GET", "/api/projects", presented, None).await;
        assert_eq!(page.status, StatusCode::UNAUTHORIZED, "{presented:?}");
        assert_eq!(page.body, r#"{"message":"Unauthenticated."}"#);
    }
    assert_eq!(app.browser().api("GET", "/api/projects", &valid, None).await.status, StatusCode::OK);
}

#[tokio::test]
async fn bare_secret_tokens_from_sanctum_still_work() {
    let app = App::new().await;
    let valid = token(&app, &["read"]).await;
    let (_, secret) = valid.split_once('|').unwrap();
    assert_eq!(app.browser().api("GET", "/api/cms", secret, None).await.status, StatusCode::OK);
}

#[tokio::test]
async fn records_last_use() {
    let app = App::new().await;
    let valid = token(&app, &["read"]).await;
    app.browser().api("GET", "/api/cms", &valid, None).await;
    let used: Option<String> = sqlx::query_scalar("SELECT last_used_at FROM personal_access_tokens").fetch_one(&app.state.db).await.unwrap();
    assert!(used.is_some());
}

#[tokio::test]
async fn upload_and_delete_need_their_permissions() {
    let app = App::new().await;
    let reader = token(&app, &["read"]).await;
    let writer = token(&app, &["create", "delete"]).await;

    let mut browser = app.browser();
    let request = |token: &str| {
        axum::http::Request::post("/api/images")
            .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
            .header(axum::http::header::CONTENT_TYPE, "multipart/form-data; boundary=b")
    };
    let body = |name: &str| {
        axum::body::Body::from(format!("--b\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\n\r\ndata\r\n--b--\r\n"))
    };

    let page = browser.send(request(&reader), body("os.img.gz")).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert_eq!(page.body, r#"{"message":"API user lacks 'create' permission"}"#);

    let page = browser.send(request(&writer), body("os.img.tar")).await;
    assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);
    let error: Value = serde_json::from_str(&page.body).unwrap();
    assert_eq!(error["errors"]["image"][0], "Only .gz, .bz2 and .xz images are supported");

    let page = browser.send(request(&writer), body("os.img.gz")).await;
    assert_eq!(page.status, StatusCode::CREATED, "Eloquent answered 201 for a newly created model");
    let image: Value = serde_json::from_str(&page.body).unwrap();
    assert_eq!((image["filename"].as_str(), image["sha256"].as_str()), (Some("os.img.gz"), Some("")));

    let id = image["id"].as_i64().unwrap();
    assert_eq!(app.browser().api("DELETE", &format!("/api/images/{id}"), &writer, None).await.status, StatusCode::OK);
    assert_eq!(app.browser().api("GET", &format!("/api/images/{id}"), &reader, None).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_validates_like_the_form() {
    let app = App::golden().await;
    scenario(&app).await;
    let token = token(&app, &["update"]).await;

    let page = app.browser().api("PATCH", "/api/projects/1", &token, Some(r#"{"storage": "/dev/sda; reboot", "image_id": 99}"#)).await;

    assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = serde_json::from_str(&page.body).unwrap();
    assert_eq!(body["message"], "The given data was invalid.");
    assert!(body["errors"]["storage"].is_array() && body["errors"]["image_id"].is_array(), "{body}");
    let storage: String = sqlx::query_scalar("SELECT storage FROM projects").fetch_one(&app.state.db).await.unwrap();
    assert_eq!(storage, "/dev/mmcblk0");
}

#[tokio::test]
async fn patching_the_active_project_restages_eeprom_firmware() {
    let app = App::golden().await;
    scenario(&app).await;
    let dir = app.path("storage/app/firmware/latest");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/eeprom/pieeprom.bin"), dir.join("pieeprom-2026-10-15.bin")).unwrap();
    let token = token(&app, &["update"]).await;
    let settings = std::str::from_utf8(include_bytes!("fixtures/eeprom/patched-settings.txt")).unwrap();
    let patch = json!({ "eeprom_firmware": "latest/pieeprom-2026-10-15.bin", "eeprom_settings": settings }).to_string();

    let page = app.browser().api("PATCH", "/api/projects/1", &token, Some(&patch)).await;

    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let staged = std::fs::read(app.path("public/uploads/pieeprom.bin")).unwrap();
    assert!(staged == include_bytes!("fixtures/eeprom/pieeprom-patched.bin"));
}
