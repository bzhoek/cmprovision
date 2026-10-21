use std::io::Write;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Multipart, State};
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::post;
use clap::Parser;
use cmprovision::config::Config;
use cmprovision::{AppState, Clock, db, images, models, web};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    config: Config,
}

async fn upload(State(state): State<AppState>, mut multipart: Multipart) -> impl IntoResponse {
    match images::store(&state, &mut multipart).await {
        Ok(image) => (StatusCode::OK, image.id.to_string()),
        Err(err) => (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()),
    }
}

async fn setup() -> (tempfile::TempDir, AppState, Router) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("db.sqlite");
    let config = Args::parse_from(["test", "--data-dir", dir.path().to_str().unwrap(), "--database", database.to_str().unwrap()]).config;
    let pool = db::connect(&database).await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = AppState::new(config, pool, Clock::System);
    let router = web::router(state.clone()).merge(Router::new().route("/upload", post(upload)).with_state(state.clone()));
    (dir, state, router)
}

fn multipart(filename: &str, contents: &[u8]) -> Request<Body> {
    let mut body = format!(
        "--b\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(contents);
    body.extend_from_slice(b"\r\n--b--\r\n");
    Request::post("/upload")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=b")
        .body(Body::from(body))
        .unwrap()
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (status, to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec())
}

async fn hashed(state: &AppState, id: i64) -> models::Image {
    for _ in 0..200 {
        let image = models::image(&state.db, Some(id)).await.unwrap().unwrap();
        if image.uncompressed_sha256.is_some() {
            return image;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("image {id} was not hashed in time");
}

#[tokio::test]
async fn upload_hash_serve_and_delete() {
    let (_dir, state, router) = setup().await;
    let disk: Vec<u8> = (0..1024 * 1024).map(|i| (i % 253) as u8).collect();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&disk).unwrap();
    let gz = gz.finish().unwrap();

    let (status, body) = send(&router, multipart("raspios.img.gz", &gz)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let id: i64 = String::from_utf8(body).unwrap().parse().unwrap();

    let image = hashed(&state, id).await;
    assert_eq!(image.filename, "raspios.img.gz");
    assert_eq!(image.filename_extension, "gz");
    assert!(image.filename_on_server.len() == 43 && image.filename_on_server.ends_with(".gz"));
    assert_eq!(image.sha256, hex::encode(Sha256::digest(&gz)));
    assert_eq!(image.uncompressed_sha256, Some(hex::encode(Sha256::digest(&disk))));
    assert_eq!(image.uncompressed_size, Some(disk.len() as i64));

    let download = Request::get(format!("/uploads/{}", image.filename_on_server)).body(Body::empty()).unwrap();
    assert_eq!(send(&router, download).await, (StatusCode::OK, gz));

    images::delete(&state, &image).await.unwrap();
    assert!(models::image(&state.db, Some(id)).await.unwrap().is_none());
    let download = Request::get(format!("/uploads/{}", image.filename_on_server)).body(Body::empty()).unwrap();
    assert_eq!(send(&router, download).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejects_unsupported_extension() {
    let (dir, _state, router) = setup().await;

    let (status, body) = send(&router, multipart("raspios.img.zip", b"zip")).await;

    assert_eq!((status, String::from_utf8(body).unwrap()), (StatusCode::UNPROCESSABLE_ENTITY, images::UNSUPPORTED.to_string()));
    assert!(!dir.path().join("public/uploads").exists() || std::fs::read_dir(dir.path().join("public/uploads")).unwrap().next().is_none());
}

#[tokio::test]
async fn resumes_pending_hashes() {
    let (dir, state, _router) = setup().await;
    let uploads = dir.path().join("public/uploads");
    std::fs::create_dir_all(&uploads).unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(b"disk").unwrap();
    std::fs::write(uploads.join("pending.gz"), gz.finish().unwrap()).unwrap();
    let id = sqlx::query("INSERT INTO images (filename, filename_on_server, filename_extension, sha256) VALUES ('p.gz', 'pending.gz', 'gz', '')")
        .execute(&state.db)
        .await
        .unwrap()
        .last_insert_rowid();

    state.hasher.enqueue_pending(&state.db).await.unwrap();

    assert_eq!(hashed(&state, id).await.uncompressed_size, Some(4));
}
