use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use clap::Parser;
use cmprovision::config::Config;
use cmprovision::{AppState, Clock, db, twofactor, web};
use tower::ServiceExt;

const EMAIL: &str = "admin@example.com";
const PASSWORD: &str = "correct horse";

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    config: Config,
}

struct App {
    _dir: tempfile::TempDir,
    state: AppState,
    router: Router,
    user_id: i64,
}

struct Browser<'a> {
    app: &'a App,
    cookie: Option<String>,
}

struct Page {
    status: StatusCode,
    location: Option<String>,
    body: String,
}

impl App {
    async fn new() -> Self {
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
        let state = AppState::new(config, pool, Clock::System);
        App { _dir: dir, router: web::router(state.clone()), state, user_id }
    }

    fn browser(&self) -> Browser<'_> {
        Browser { app: self, cookie: None }
    }

    async fn enable_two_factor(&self) -> (String, Vec<String>) {
        let secret = twofactor::generate_secret();
        let codes = twofactor::generate_recovery_codes();
        twofactor::enable(&self.state.db, self.user_id, &secret, &codes).await.unwrap();
        (secret, codes)
    }
}

impl Browser<'_> {
    async fn send(&mut self, request: axum::http::request::Builder, body: Body) -> Page {
        let mut request = request.header(header::HOST, "cmprovision.local");
        if let Some(cookie) = &self.cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let response = self.app.router.clone().oneshot(request.body(body).unwrap()).await.unwrap();
        if let Some(set) = response.headers().get(header::SET_COOKIE) {
            self.cookie = Some(set.to_str().unwrap().split(';').next().unwrap().to_string());
        }
        let status = response.status();
        let location = response.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
        let body = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        Page { status, location, body }
    }

    async fn get(&mut self, path: &str) -> Page {
        self.send(Request::get(path), Body::empty()).await
    }

    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Page {
        let body = form_urlencoded::Serializer::new(String::new()).extend_pairs(form).finish();
        let request = Request::post(path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ORIGIN, "http://cmprovision.local");
        self.send(request, Body::from(body)).await
    }

    async fn login(&mut self) -> Page {
        self.get("/login").await;
        self.post("/login", &[("email", EMAIL), ("password", PASSWORD)]).await
    }
}

fn current_code(secret: &str) -> String {
    totp_rs::Builder::new()
        .with_secret(totp_rs::Secret::try_from_base32(secret).unwrap())
        .with_account_name(EMAIL)
        .build()
        .unwrap()
        .generate_current()
        .to_string()
}

#[tokio::test]
async fn guests_are_sent_to_login_and_back() {
    let app = App::new().await;
    let mut browser = app.browser();

    let page = browser.get("/user/profile").await;
    assert_eq!((page.status, page.location.as_deref()), (StatusCode::SEE_OTHER, Some("/login")));

    let page = browser.post("/login", &[("email", EMAIL), ("password", PASSWORD)]).await;
    assert_eq!(page.location.as_deref(), Some("/user/profile"));
    assert!(browser.get("/user/profile").await.body.contains("Profile information"));
}

#[tokio::test]
async fn login_with_laravel_password_hash() {
    let app = App::new().await;
    let mut browser = app.browser();

    let page = browser.login().await;
    assert_eq!(page.location.as_deref(), Some("/dashboard"));

    let page = browser.get("/dashboard").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Provisioning log") && page.body.contains("Admin"));
}

#[tokio::test]
async fn wrong_password_is_refused_then_rate_limited() {
    let app = App::new().await;
    let mut browser = app.browser();

    for _ in 0..5 {
        let page = browser.post("/login", &[("email", EMAIL), ("password", "wrong")]).await;
        assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(page.body.contains("These credentials do not match our records."));
    }
    let page = browser.post("/login", &[("email", EMAIL), ("password", PASSWORD)]).await;
    assert!(page.body.contains("Too many login attempts"), "{}", page.body);
}

#[tokio::test]
async fn unknown_email_gets_the_same_answer() {
    let app = App::new().await;
    let page = app.browser().post("/login", &[("email", "nobody@example.com"), ("password", PASSWORD)]).await;
    assert!(page.body.contains("These credentials do not match our records."));
}

#[tokio::test]
async fn cross_site_posts_are_refused() {
    let app = App::new().await;
    let mut browser = app.browser();
    browser.login().await;

    let request = Request::post("/logout").header(header::ORIGIN, "http://evil.example");
    assert_eq!(browser.send(request, Body::empty()).await.status, StatusCode::FORBIDDEN);
    let request = Request::post("/logout").header("sec-fetch-site", "cross-site");
    assert_eq!(browser.send(request, Body::empty()).await.status, StatusCode::FORBIDDEN);
    assert_eq!(browser.get("/dashboard").await.status, StatusCode::OK);
}

#[tokio::test]
async fn logout_ends_the_session() {
    let app = App::new().await;
    let mut browser = app.browser();
    browser.login().await;

    browser.post("/logout", &[]).await;

    assert_eq!(browser.get("/dashboard").await.location.as_deref(), Some("/login"));
}

#[tokio::test]
async fn login_cycles_the_session_id() {
    let app = App::new().await;
    let mut browser = app.browser();
    browser.get("/login").await;
    browser.get("/dashboard").await;
    let before = browser.cookie.clone();

    browser.post("/login", &[("email", EMAIL), ("password", PASSWORD)]).await;

    assert!(before.is_some());
    assert_ne!(browser.cookie, before);
}

#[tokio::test]
async fn two_factor_code_is_required_and_single_use() {
    let app = App::new().await;
    let (secret, _) = app.enable_two_factor().await;
    let mut browser = app.browser();

    let page = browser.login().await;
    assert_eq!(page.location.as_deref(), Some("/two-factor-challenge"));
    assert_eq!(browser.get("/dashboard").await.location.as_deref(), Some("/login"));

    let code = current_code(&secret);
    let page = browser.post("/two-factor-challenge", &[("code", &code)]).await;
    assert_eq!(page.location.as_deref(), Some("/dashboard"));

    let mut eavesdropper = app.browser();
    eavesdropper.login().await;
    let page = eavesdropper.post("/two-factor-challenge", &[("code", &code)]).await;
    assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn recovery_codes_work_once() {
    let app = App::new().await;
    let (_, codes) = app.enable_two_factor().await;

    let mut browser = app.browser();
    browser.login().await;
    let page = browser.post("/two-factor-challenge", &[("recovery_code", &codes[0])]).await;
    assert_eq!(page.location.as_deref(), Some("/dashboard"));

    let mut again = app.browser();
    again.login().await;
    let page = again.post("/two-factor-challenge", &[("recovery_code", &codes[0])]).await;
    assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn enabling_two_factor_needs_password_and_a_valid_code() {
    let app = App::new().await;
    let mut browser = app.browser();
    browser.login().await;

    let page = browser.post("/user/two-factor/enable", &[("password", "wrong")]).await;
    assert!(page.body.contains("The provided password was incorrect."));

    browser.post("/user/two-factor/enable", &[("password", PASSWORD)]).await;
    let page = browser.get("/user/profile").await;
    let secret = page.body.split("Setup key: <span class=\"font-mono\">").nth(1).unwrap().split('<').next().unwrap().to_string();
    assert!(page.body.contains("data:image/png;base64,"));

    let page = browser.post("/user/two-factor/confirm", &[("code", "000000")]).await;
    assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);

    browser.post("/user/two-factor/confirm", &[("code", &current_code(&secret))]).await;
    let page = browser.get("/user/profile").await;
    assert!(page.body.contains("Two factor authentication is <strong>enabled</strong>"));
    let user = cmprovision::auth::user(&app.state.db, app.user_id).await.unwrap().unwrap();
    let codes = twofactor::recovery_codes(&user);
    assert_eq!(codes.len(), 8);
    assert!(codes.iter().all(|c| page.body.contains(c.as_str())), "recovery codes shown after enabling");
    assert!(!browser.get("/user/profile").await.body.contains(codes[0].as_str()), "and only once");
}

#[tokio::test]
async fn password_change_validates_and_applies() {
    let app = App::new().await;
    let mut browser = app.browser();
    browser.login().await;

    let page = browser.post("/user/password", &[("current_password", PASSWORD), ("password", "short"), ("password_confirmation", "short")]).await;
    assert!(page.body.contains("at least 8 characters"));

    let page = browser
        .post("/user/password", &[("current_password", PASSWORD), ("password", "a new password"), ("password_confirmation", "a new password")])
        .await;
    assert_eq!(page.location.as_deref(), Some("/user/profile"));

    browser.post("/logout", &[]).await;
    let page = browser.post("/login", &[("email", EMAIL), ("password", "a new password")]).await;
    assert_eq!(page.location.as_deref(), Some("/dashboard"));
}

#[tokio::test]
async fn api_token_is_shown_once_in_sanctum_format() {
    let app = App::new().await;
    let mut browser = app.browser();
    browser.login().await;

    browser.post("/user/api-tokens", &[("name", "ci"), ("read", "on"), ("create", "on")]).await;
    let page = browser.get("/user/api-tokens").await;
    let token = page.body.split("readonly value=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    let (id, secret) = token.split_once('|').unwrap();
    assert_eq!(secret.len(), 40);

    let (stored, abilities): (String, String) = sqlx::query_as("SELECT token, abilities FROM personal_access_tokens WHERE id = ?")
        .bind(id.parse::<i64>().unwrap())
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(stored, hex::encode(<sha2::Sha256 as sha2::Digest>::digest(secret)));
    assert_eq!(abilities, r#"["create","read"]"#);
    assert!(!browser.get("/user/api-tokens").await.body.contains(secret));
}
