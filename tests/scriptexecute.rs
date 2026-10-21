//! Replays the scenarios of tests/Golden/CaptureTest.php against the Rust server and
//! compares responses and database state with the outputs captured from the PHP app.

use std::net::SocketAddr;
use std::sync::LazyLock;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, header};
use chrono::NaiveDate;
use clap::Parser;
use cmprovision::config::Config;
use cmprovision::{AppState, Clock, db, web};
use regex::Regex;
use serde_json::{Map, Value, json};
use sqlx::{Column, Row, SqlitePool, TypeInfo, ValueRef};
use tower::ServiceExt;

const SERIAL: &str = "10000000abcdef12";
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scriptexecute");
const DEVICE_IP: &str = "172.20.0.5";

static RANDOM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^echo "[A-Za-z0-9]{64}" >/dev/urandom$"#).unwrap());
static LABEL_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"'[^' ]*/label-[A-Za-z0-9]+'").unwrap());

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    config: Config,
}

struct Server {
    _dir: tempfile::TempDir,
    db: SqlitePool,
    router: Router,
    host: &'static str,
}

impl Server {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("db.sqlite");
        let config = Args::parse_from(["test", "--data-dir", dir.path().to_str().unwrap(), "--database", database.to_str().unwrap()]).config;
        let db = db::connect(&database).await.unwrap();
        db::migrate(&db).await.unwrap();
        // The PHP capture ran on an unseeded database, so ids start at 1 there.
        sqlx::raw_sql("DELETE FROM scripts; DELETE FROM labels; DELETE FROM sqlite_sequence;")
            .execute(&db)
            .await
            .unwrap();
        let clock = Clock::Fixed(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap().and_hms_opt(3, 4, 5).unwrap());
        let router = web::router(AppState::new(config, db.clone(), clock));
        Server { _dir: dir, db, router, host: "172.20.0.1" }
    }

    async fn exec(&self, sql: &str, binds: &[Value]) -> i64 {
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.to_string()));
        for bind in binds {
            query = match bind {
                Value::Null => query.bind(None::<String>),
                Value::Bool(b) => query.bind(*b),
                Value::Number(n) => query.bind(n.as_i64().unwrap()),
                Value::String(s) => query.bind(s.clone()),
                _ => unreachable!(),
            };
        }
        query.execute(&self.db).await.unwrap().last_insert_rowid()
    }

    async fn image(&self, ext: &str, size: Option<i64>, sha: Option<&str>) -> i64 {
        self.exec(
            "INSERT INTO images (filename, filename_on_server, filename_extension, sha256, uncompressed_sha256, uncompressed_size) VALUES (?, ?, ?, ?, ?, ?)",
            &[
                json!(format!("raspios.img.{ext}")),
                json!(format!("abcdefghijklmnopqrstuvwxyz0123456789ABCD.{ext}")),
                json!(ext),
                json!(format!("bb{}", "0".repeat(62))),
                json!(sha),
                json!(size),
            ],
        )
        .await
    }

    async fn default_image(&self, ext: &str) -> i64 {
        self.image(ext, Some(1073741824), Some(&format!("aa{}", "0".repeat(62)))).await
    }

    async fn script(&self, name: &str, script_type: &str, priority: i64, bg: bool) -> i64 {
        self.exec(
            "INSERT INTO scripts (name, script_type, priority, bg, script) VALUES (?, ?, ?, ?, ?)",
            &[json!(name), json!(script_type), json!(priority), json!(bg), json!(format!("#!/bin/sh\necho '{name}' \"$STORAGE\"\n"))],
        )
        .await
    }

    async fn project(&self, attrs: Value, scripts: &[i64]) -> i64 {
        let mut p = json!({"name": "Golden", "device": "cm4", "storage": "/dev/mmcblk0", "label_moment": "never", "verify": false, "image_id": null, "label_id": null, "eeprom_firmware": null});
        p.as_object_mut().unwrap().extend(attrs.as_object().unwrap().clone());
        let id = self
            .exec(
                "INSERT INTO projects (name, device, storage, label_moment, verify, image_id, label_id, eeprom_firmware) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &[p["name"].clone(), p["device"].clone(), p["storage"].clone(), p["label_moment"].clone(), p["verify"].clone(), p["image_id"].clone(), p["label_id"].clone(), p["eeprom_firmware"].clone()],
            )
            .await;
        for script in scripts {
            self.exec("INSERT INTO project_script (project_id, script_id) VALUES (?, ?)", &[json!(id), json!(script)]).await;
        }
        self.setting("active_project", &id.to_string()).await;
        id
    }

    async fn setting(&self, key: &str, value: &str) {
        self.exec(r#"INSERT OR REPLACE INTO settings ("key", "value") VALUES (?, ?)"#, &[json!(key), json!(value)]).await;
    }

    async fn request(&self, method: &str, query: &str, files: &[(&str, &str)]) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/scriptexecute?{query}"))
            .header(header::HOST, self.host);
        let body = if files.is_empty() {
            Body::empty()
        } else {
            let boundary = "golden-boundary";
            request = request.header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"));
            let mut body = String::new();
            for (name, contents) in files {
                body.push_str(&format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n{contents}\r\n"
                ));
            }
            body.push_str(&format!("--{boundary}--\r\n"));
            Body::from(body)
        };
        let mut request = request.body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([172, 20, 0, 5], 40000))));

        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let content_type = response.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        Response { status, content_type, body: String::from_utf8(body.to_vec()).unwrap() }
    }

    async fn start(&self, extra: &[(&str, &str)]) -> Response {
        let mut params: Vec<(&str, &str)> = extra.to_vec();
        for default in [
            ("serial", SERIAL),
            ("model", "CM4"),
            ("memorysize", "3884000"),
            ("storagesize", "61071360"),
            ("cid", "15010038474e443352"),
            ("csd", "d00f00320f5903ff"),
            ("temp", "45.2"),
        ] {
            if !params.iter().any(|(k, _)| *k == default.0) {
                params.push(default);
            }
        }
        let query = form_urlencoded::Serializer::new(String::new()).extend_pairs(params).finish();
        self.request("GET", &query, &[]).await
    }

    async fn state(&self) -> Value {
        let cm = sqlx::query("SELECT * FROM cms WHERE serial = ?").bind(SERIAL).fetch_optional(&self.db).await.unwrap();
        let cm = cm.map(|row| {
            let mut obj = row_to_json(&row);
            for skip in ["id", "created_at", "updated_at"] {
                obj.remove(skip);
            }
            Value::Object(obj)
        });
        let logs: Vec<Value> = sqlx::query("SELECT cm, board, ip, loglevel, msg FROM cmlogs ORDER BY id")
            .fetch_all(&self.db)
            .await
            .unwrap()
            .iter()
            .map(|row| {
                let mut obj = row_to_json(row);
                let msg = obj["msg"].as_str().unwrap();
                obj.insert("msg".into(), json!(LABEL_FILE.replace_all(msg, "'LABELFILE'")));
                Value::Object(obj)
            })
            .collect();
        json!({"cm": cm, "cmlogs": logs})
    }
}

struct Response {
    status: u16,
    content_type: Option<String>,
    body: String,
}

fn row_to_json(row: &sqlx::sqlite::SqliteRow) -> Map<String, Value> {
    row.columns()
        .iter()
        .map(|col| {
            let raw = row.try_get_raw(col.ordinal()).unwrap();
            let value = if raw.is_null() {
                Value::Null
            } else {
                match raw.type_info().name() {
                    "INTEGER" => json!(row.get::<i64, _>(col.ordinal())),
                    "REAL" => json!(row.get::<f64, _>(col.ordinal())),
                    _ => json!(row.get::<String, _>(col.ordinal())),
                }
            };
            (col.name().to_string(), value)
        })
        .collect()
}

async fn assert_golden(server: &Server, name: &str, response: Response) {
    let mut expected: Value = serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/{name}.state.json")).unwrap()).unwrap();
    // PHP silently dropped the client IP (not mass-assignable); the rewrite records it.
    for log in expected["cmlogs"].as_array_mut().unwrap() {
        log["ip"] = json!(DEVICE_IP);
    }

    assert_eq!(response.status, expected["status"], "{name}: status");
    assert_eq!(json!(response.content_type), expected["content_type"], "{name}: content type");
    if response.status == 200 {
        let golden = std::fs::read_to_string(format!("{FIXTURES}/{name}.sh")).unwrap();
        let body = RANDOM.replace(&response.body, r#"echo "RANDOM64" >/dev/urandom"#);
        assert_eq!(body, golden, "{name}: body");
    }
    let state = server.state().await;
    assert_eq!(state["cm"], expected["cm"], "{name}: cm row");
    assert_eq!(state["cmlogs"], expected["cmlogs"], "{name}: cmlogs");
}

#[tokio::test]
async fn missing_serial() {
    let s = Server::new().await;
    let r = s.request("GET", "", &[]).await;
    assert_golden(&s, "missing_serial", r).await;
}

#[tokio::test]
async fn no_project() {
    let s = Server::new().await;
    let r = s.start(&[]).await;
    assert_golden(&s, "no_project", r).await;
}

#[tokio::test]
async fn gz_with_scripts() {
    let s = Server::new().await;
    let image = s.default_image("gz").await;
    let scripts = [
        s.script("pre-b", "preinstall", 50, false).await,
        s.script("pre-a", "preinstall", 50, false).await,
        s.script("pre-late", "preinstall", 200, false).await,
        s.script("post-bg", "postinstall", 100, true).await,
    ];
    s.project(json!({"image_id": image}), &scripts).await;
    let r = s.start(&[("mac", "dc:a6:32:01:02:03")]).await;
    assert_golden(&s, "gz_with_scripts", r).await;
}

#[tokio::test]
async fn xz_bootmode3_inversejumper() {
    let s = Server::new().await;
    let image = s.default_image("xz").await;
    s.project(json!({"image_id": image}), &[]).await;
    let r = s.start(&[("bootmode", "3"), ("inversejumper", "1101")]).await;
    assert_golden(&s, "xz_bootmode3_inversejumper", r).await;
}

#[tokio::test]
async fn bz2_sda_without_storagesize() {
    let s = Server::new().await;
    let image = s.default_image("bz2").await;
    s.project(json!({"image_id": image, "storage": "/dev/sda"}), &[]).await;
    let r = s.start(&[("storagesize", "")]).await;
    assert_golden(&s, "bz2_sda_without_storagesize", r).await;
}

#[tokio::test]
async fn scripts_without_image() {
    let s = Server::new().await;
    let script = s.script("post-only", "postinstall", 100, false).await;
    s.project(json!({}), &[script]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "scripts_without_image", r).await;
}

#[tokio::test]
async fn eeprom_and_verify_mib_aligned() {
    let s = Server::new().await;
    s.setting("active_eeprom_sha256", &format!("cc{}", "0".repeat(62))).await;
    let image = s.default_image("gz").await;
    let script = s.script("post-a", "postinstall", 100, false).await;
    s.project(json!({"image_id": image, "eeprom_firmware": "stable/pieeprom-2024-01-01.bin", "verify": true}), &[script]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "eeprom_and_verify_mib_aligned", r).await;
}

#[tokio::test]
async fn verify_sector_aligned() {
    let s = Server::new().await;
    let image = s.image("xz", Some(1048576 + 512), Some(&format!("aa{}", "0".repeat(62)))).await;
    s.project(json!({"image_id": image, "verify": true}), &[]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "verify_sector_aligned", r).await;
}

#[tokio::test]
async fn ipv6_host() {
    let mut s = Server::new().await;
    s.host = "[fe80::1]";
    let image = s.default_image("gz").await;
    s.project(json!({"image_id": image}), &[]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "ipv6_host", r).await;
}

#[tokio::test]
async fn label_preinstall_command_failure() {
    let s = Server::new().await;
    let label = s
        .exec(
            "INSERT INTO labels (name, printer_type, print_command, file_extension, template) VALUES ('Cmd', 'command', 'false $file', 'txt', '$mac $serial $provisionboard')",
            &[],
        )
        .await;
    let image = s.default_image("gz").await;
    s.project(json!({"image_id": image, "label_id": label, "label_moment": "preinstall"}), &[]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "label_preinstall_command_failure", r).await;
}

#[tokio::test]
async fn error_verify_without_uncompressed_sha() {
    let s = Server::new().await;
    let image = s.image("gz", None, None).await;
    s.project(json!({"image_id": image, "verify": true}), &[]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "error_verify_without_uncompressed_sha", r).await;
}

#[tokio::test]
async fn error_not_sector_multiple() {
    let s = Server::new().await;
    let image = s.image("gz", Some(1000), Some(&format!("aa{}", "0".repeat(62)))).await;
    s.project(json!({"image_id": image, "verify": true}), &[]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "error_not_sector_multiple", r).await;
}

#[tokio::test]
async fn error_missing_emmc() {
    let s = Server::new().await;
    let image = s.default_image("gz").await;
    s.project(json!({"image_id": image}), &[]).await;
    let r = s.start(&[("storagesize", "")]).await;
    assert_golden(&s, "error_missing_emmc", r).await;
}

#[tokio::test]
async fn error_image_too_big() {
    let s = Server::new().await;
    let image = s.image("gz", Some(64 * 1024 * 1024 * 1024), Some(&format!("aa{}", "0".repeat(62)))).await;
    s.project(json!({"image_id": image}), &[]).await;
    let r = s.start(&[]).await;
    assert_golden(&s, "error_image_too_big", r).await;
}

#[tokio::test]
async fn followup_requests() {
    let s = Server::new().await;
    let image = s.default_image("gz").await;
    let script = s.script("pre-a", "preinstall", 100, false).await;
    s.project(json!({"image_id": image}), &[script]).await;
    s.start(&[]).await;

    let serial = format!("serial={SERIAL}");
    let r = s.request("POST", &serial, &[("eeprom_version", "VERSION:abcdef\nBUILD_TIMESTAMP=1700000000\n")]).await;
    assert_golden(&s, "followup_eeprom_version", r).await;

    let r = s.request("POST", &format!("{serial}&retcode=0&phase=preinstall"), &[("log", "pre output\n")]).await;
    assert_golden(&s, "followup_preinstall_ok", r).await;

    let r = s.request("POST", &format!("{serial}&retcode=1&phase=dd"), &[("log", "dd: error writing\n")]).await;
    assert_golden(&s, "followup_dd_failed", r).await;

    let r = s.request("GET", &format!("{serial}&alldone=1&temp=50.1&verify=1"), &[]).await;
    assert_golden(&s, "followup_alldone", r).await;
}

#[tokio::test]
async fn followup_unknown_serial() {
    let s = Server::new().await;
    let r = s.request("GET", "serial=deadbeef&alldone=1", &[]).await;
    assert_golden(&s, "followup_unknown_serial", r).await;
}

#[tokio::test]
async fn rejects_unsafe_serial_and_host() {
    let mut s = Server::new().await;
    assert_eq!(s.request("GET", "serial=abc%22%3Breboot", &[]).await.status, 401);
    s.host = "evil;reboot";
    assert_eq!(s.start(&[]).await.status, 400);
}

#[tokio::test]
async fn explains_unprepared_eeprom_firmware() {
    let s = Server::new().await;
    s.project(json!({"eeprom_firmware": "stable/pieeprom-2023-01-11.bin"}), &[]).await;

    let r = s.start(&[]).await;

    assert_eq!(r.status, 200);
    assert!(r.body.starts_with("echo 'EEPROM firmware of the active project is not prepared."), "{}", r.body);
    assert_eq!(s.state().await["cmlogs"][0]["loglevel"], "error");
}

#[tokio::test]
async fn truncates_oversized_logs() {
    let s = Server::new().await;
    s.project(json!({}), &[]).await;
    s.start(&[]).await;
    let log = "x".repeat(1024 * 1024 + 10);

    s.request("POST", &format!("serial={SERIAL}&retcode=0&phase=preinstall"), &[("log", &log)]).await;

    let stored: String = sqlx::query_scalar("SELECT pre_script_output FROM cms").fetch_one(&s.db).await.unwrap();
    assert_eq!(stored, format!("{}\nLog was bigger than max allowed size. Truncated.", "x".repeat(1024 * 1024)));
}
