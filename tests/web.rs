use axum::body::Body;
use axum::http::{Request, StatusCode};
use clap::Parser;
use cmprovision::config::Config;
use cmprovision::{AppState, db, web};
use tower::ServiceExt;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    config: Config,
}

#[tokio::test]
async fn healthz_responds() {
    let dir = tempfile::tempdir().unwrap();
    let config = Args::parse_from(["test", "--data-dir", dir.path().to_str().unwrap(), "--database", dir.path().join("db.sqlite").to_str().unwrap()]).config;
    let pool = db::connect(&config.database()).await.unwrap();
    db::migrate(&pool).await.unwrap();

    let response = web::router(AppState { config, db: pool })
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
