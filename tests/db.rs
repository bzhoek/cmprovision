use std::path::Path;

use cmprovision::db;
use sqlx::SqlitePool;

async fn schema(pool: &SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT name, sql FROM sqlite_master \
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx_%' \
         ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn count(pool: &SqlitePool, table: &'static str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

fn copy_laravel_fixture(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("laravel.sqlite");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/laravel.sqlite"), &path).unwrap();
    path
}

#[tokio::test]
async fn fresh_database_is_created_and_seeded() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("new.sqlite")).await.unwrap();

    db::migrate(&pool).await.unwrap();

    assert_eq!(count(&pool, "labels").await, 1);
    assert_eq!(count(&pool, "scripts").await, 3);
}

#[tokio::test]
async fn seed_matches_laravel_seeders() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = db::connect(&dir.path().join("new.sqlite")).await.unwrap();
    db::migrate(&fresh).await.unwrap();
    let laravel = db::connect(&copy_laravel_fixture(dir.path())).await.unwrap();

    type Script = (i64, String, String, i64, i64, String);
    let scripts = "SELECT id, name, script_type, bg, priority, script FROM scripts ORDER BY id";
    let fresh_scripts: Vec<Script> = sqlx::query_as(scripts).fetch_all(&fresh).await.unwrap();
    let laravel_scripts: Vec<Script> = sqlx::query_as(scripts).fetch_all(&laravel).await.unwrap();
    assert_eq!(fresh_scripts, laravel_scripts);

    type Label = (i64, String, String, String, String, String, String, String, String);
    let labels = "SELECT id, name, printer_type, print_command, ftp_hostname, ftp_username, ftp_password, file_extension, template FROM labels ORDER BY id";
    let fresh_labels: Vec<Label> = sqlx::query_as(labels).fetch_all(&fresh).await.unwrap();
    let laravel_labels: Vec<Label> = sqlx::query_as(labels).fetch_all(&laravel).await.unwrap();
    assert_eq!(fresh_labels, laravel_labels);
}

#[tokio::test]
async fn migrate_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("new.sqlite")).await.unwrap();

    db::migrate(&pool).await.unwrap();
    db::migrate(&pool).await.unwrap();

    assert_eq!(count(&pool, "scripts").await, 3);
}

#[tokio::test]
async fn laravel_database_is_adopted_without_reseeding() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&copy_laravel_fixture(dir.path())).await.unwrap();
    sqlx::query("DELETE FROM scripts WHERE name = 'Resize ext4 partition'")
        .execute(&pool)
        .await
        .unwrap();

    db::migrate(&pool).await.unwrap();

    assert_eq!(count(&pool, "scripts").await, 2);
    assert_eq!(count(&pool, "labels").await, 1);
}

#[tokio::test]
async fn baseline_schema_matches_laravel() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = db::connect(&dir.path().join("new.sqlite")).await.unwrap();
    db::migrate(&fresh).await.unwrap();
    let laravel = db::connect(&copy_laravel_fixture(dir.path())).await.unwrap();

    let normalize = |rows: Vec<(String, String)>| -> Vec<(String, String)> {
        rows.into_iter()
            .map(|(name, sql)| (name, sql.replace(" IF NOT EXISTS", "")))
            .collect()
    };
    let laravel_only = ["failed_jobs", "jobs", "migrations", "password_resets", "sessions"];
    let expected: Vec<_> = normalize(schema(&laravel).await)
        .into_iter()
        .filter(|(name, _)| !laravel_only.iter().any(|t| name == t || name.starts_with(&format!("{t}_"))))
        .collect();

    assert_eq!(normalize(schema(&fresh).await), expected);
}
