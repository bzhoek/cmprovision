//! The user's own account: profile details, password, two-factor authentication and API tokens.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::auth::{self, CurrentUser, User};
use crate::templates::{self, ApiTokens, Errors, Nav, Profile, TokenRow};
use crate::web::Result;
use crate::{AppState, tokens, twofactor};

const PENDING_SECRET: &str = "profile.pending_two_factor_secret";
const SHOW_CODES: &str = "profile.show_recovery_codes";
const NEW_TOKEN: &str = "profile.new_token";
const MIN_PASSWORD: usize = 8;

struct ProfilePage {
    name: String,
    email: String,
    profile_errors: Errors,
    password_errors: Errors,
    two_factor_error: Option<String>,
    status: StatusCode,
}

impl ProfilePage {
    fn of(user: &User) -> Self {
        ProfilePage {
            name: user.name.clone(),
            email: user.email.clone(),
            profile_errors: Errors::default(),
            password_errors: Errors::default(),
            two_factor_error: None,
            status: StatusCode::OK,
        }
    }

    fn failed(mut self) -> Self {
        self.status = StatusCode::UNPROCESSABLE_ENTITY;
        self
    }

    async fn render(self, session: &Session, user: &User) -> Result<Response> {
        let pending: Option<String> = session.get(PENDING_SECRET).await?;
        let show_codes: bool = session.remove(SHOW_CODES).await?.unwrap_or(false);
        let page = Profile {
            nav: Nav::new(session, user, "profile").await?,
            name: self.name,
            email: self.email,
            profile_errors: self.profile_errors,
            password_errors: self.password_errors,
            two_factor_error: self.two_factor_error,
            two_factor_enabled: user.has_two_factor(),
            pending_qr: pending.as_deref().and_then(|s| twofactor::qr_code(s, &user.email)),
            pending_secret: pending,
            recovery_codes: if show_codes { twofactor::recovery_codes(user) } else { Vec::new() },
        };
        templates::render_status(self.status, page)
    }
}

pub async fn show(CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    ProfilePage::of(&user).render(&session, &user).await
}

#[derive(Deserialize)]
pub struct ProfileForm {
    name: String,
    email: String,
}

fn looks_like_email(email: &str) -> bool {
    email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty() && !email.contains(char::is_whitespace))
}

pub async fn update_profile(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(form): Form<ProfileForm>,
) -> Result<Response> {
    let mut page = ProfilePage::of(&user);
    page.name = form.name.trim().to_string();
    page.email = form.email.trim().to_string();
    if page.name.is_empty() || page.name.len() > 255 {
        page.profile_errors.add("name", "The name field is required and may not be longer than 255 characters.");
    }
    if !looks_like_email(&page.email) || page.email.len() > 255 {
        page.profile_errors.add("email", "The email must be a valid email address.");
    } else if auth::user_by_email(&state.db, &page.email).await?.is_some_and(|other| other.id != user.id) {
        page.profile_errors.add("email", "The email has already been taken.");
    }
    if !page.profile_errors.is_empty() {
        return page.failed().render(&session, &user).await;
    }

    sqlx::query("UPDATE users SET name = ?, email = ?, updated_at = ? WHERE id = ?")
        .bind(&page.name)
        .bind(&page.email)
        .bind(state.clock.timestamp())
        .bind(user.id)
        .execute(&state.db)
        .await?;
    auth::flash(&session, "Profile saved.").await?;
    Ok(Redirect::to("/user/profile").into_response())
}

#[derive(Deserialize)]
pub struct PasswordForm {
    current_password: String,
    password: String,
    password_confirmation: String,
}

pub async fn update_password(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(form): Form<PasswordForm>,
) -> Result<Response> {
    let mut page = ProfilePage::of(&user);
    if !auth::verify_password(&form.current_password, &user.password).await {
        page.password_errors.add("current_password", "The provided password does not match your current password.");
    }
    if form.password.chars().count() < MIN_PASSWORD {
        page.password_errors.add("password", format!("The password must be at least {MIN_PASSWORD} characters."));
    } else if form.password != form.password_confirmation {
        page.password_errors.add("password", "The password confirmation does not match.");
    }
    if !page.password_errors.is_empty() {
        return page.failed().render(&session, &user).await;
    }

    sqlx::query("UPDATE users SET password = ?, updated_at = ? WHERE id = ?")
        .bind(auth::hash_password(&form.password).await?)
        .bind(state.clock.timestamp())
        .bind(user.id)
        .execute(&state.db)
        .await?;
    auth::flash(&session, "Password changed.").await?;
    Ok(Redirect::to("/user/profile").into_response())
}

#[derive(Deserialize)]
pub struct ConfirmPassword {
    password: String,
}

/// Two-factor changes need the password, like Fortify's confirmPassword option.
async fn reconfirm(user: &User, form: &ConfirmPassword, session: &Session) -> Result<Option<Response>> {
    if auth::verify_password(&form.password, &user.password).await {
        return Ok(None);
    }
    let mut page = ProfilePage::of(user);
    page.two_factor_error = Some("The provided password was incorrect.".into());
    Ok(Some(page.failed().render(session, user).await?))
}

pub async fn enable_two_factor(CurrentUser(user): CurrentUser, session: Session, Form(form): Form<ConfirmPassword>) -> Result<Response> {
    if let Some(refused) = reconfirm(&user, &form, &session).await? {
        return Ok(refused);
    }
    session.insert(PENDING_SECRET, twofactor::generate_secret()).await?;
    Ok(Redirect::to("/user/profile#two-factor").into_response())
}

#[derive(Deserialize)]
pub struct CodeForm {
    code: String,
}

/// Only stores the secret once the user proves their app generates matching codes.
pub async fn confirm_two_factor(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(form): Form<CodeForm>,
) -> Result<Response> {
    let Some(secret) = session.get::<String>(PENDING_SECRET).await? else {
        return Ok(Redirect::to("/user/profile").into_response());
    };
    if !twofactor::verify(&secret, &user.email, form.code.trim()) {
        let mut page = ProfilePage::of(&user);
        page.two_factor_error = Some(auth::BAD_CODE.into());
        return page.failed().render(&session, &user).await;
    }
    twofactor::enable(&state.db, user.id, &secret, &twofactor::generate_recovery_codes()).await?;
    session.remove::<String>(PENDING_SECRET).await?;
    session.insert(SHOW_CODES, true).await?;
    auth::flash(&session, "Two factor authentication enabled.").await?;
    Ok(Redirect::to("/user/profile#two-factor").into_response())
}

pub async fn cancel_two_factor(session: Session) -> Result<Redirect> {
    session.remove::<String>(PENDING_SECRET).await?;
    Ok(Redirect::to("/user/profile#two-factor"))
}

pub async fn disable_two_factor(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(form): Form<ConfirmPassword>,
) -> Result<Response> {
    if let Some(refused) = reconfirm(&user, &form, &session).await? {
        return Ok(refused);
    }
    twofactor::disable(&state.db, user.id).await?;
    auth::flash(&session, "Two factor authentication disabled.").await?;
    Ok(Redirect::to("/user/profile#two-factor").into_response())
}

pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(form): Form<ConfirmPassword>,
) -> Result<Response> {
    if let Some(refused) = reconfirm(&user, &form, &session).await? {
        return Ok(refused);
    }
    if user.has_two_factor() {
        twofactor::replace_recovery_codes(&state.db, user.id, &twofactor::generate_recovery_codes()).await?;
        session.insert(SHOW_CODES, true).await?;
    }
    Ok(Redirect::to("/user/profile#two-factor").into_response())
}

async fn tokens_page(state: &AppState, session: &Session, user: &User, name: String, errors: Errors, status: StatusCode) -> Result<Response> {
    let tokens = tokens::list(&state.db, user.id)
        .await?
        .into_iter()
        .map(|t| TokenRow { id: t.id, abilities: t.abilities(), name: t.name, last_used_at: t.last_used_at })
        .collect();
    let page = ApiTokens {
        nav: Nav::new(session, user, "api-tokens").await?,
        tokens,
        permissions: tokens::PERMISSIONS,
        defaults: tokens::DEFAULT_PERMISSIONS,
        new_token: session.remove(NEW_TOKEN).await?,
        name,
        errors,
    };
    templates::render_status(status, page)
}

pub async fn api_tokens(State(state): State<AppState>, CurrentUser(user): CurrentUser, session: Session) -> Result<Response> {
    tokens_page(&state, &session, &user, String::new(), Errors::default(), StatusCode::OK).await
}

/// Checkboxes named after each permission; absent means unchecked.
#[derive(Deserialize)]
pub struct PermissionsForm {
    name: Option<String>,
    create: Option<String>,
    read: Option<String>,
    update: Option<String>,
    delete: Option<String>,
}

impl PermissionsForm {
    fn permissions(&self) -> Vec<&'static str> {
        [("create", &self.create), ("read", &self.read), ("update", &self.update), ("delete", &self.delete)]
            .into_iter()
            .filter(|(_, checked)| checked.is_some())
            .map(|(p, _)| p)
            .collect()
    }
}

pub async fn create_api_token(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Form(form): Form<PermissionsForm>,
) -> Result<Response> {
    let name = form.name.as_deref().unwrap_or_default().trim().to_string();
    if name.is_empty() || name.len() > 255 {
        let mut errors = Errors::default();
        errors.add("name", "The name field is required and may not be longer than 255 characters.");
        return tokens_page(&state, &session, &user, name, errors, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
    let token = tokens::create(&state.db, &state.clock, user.id, &name, &form.permissions()).await?;
    session.insert(NEW_TOKEN, token).await?;
    Ok(Redirect::to("/user/api-tokens").into_response())
}

pub async fn update_api_token(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Path(id): Path<i64>,
    Form(form): Form<PermissionsForm>,
) -> Result<Redirect> {
    tokens::update_permissions(&state.db, &state.clock, user.id, id, &form.permissions()).await?;
    auth::flash(&session, "Permissions saved.").await?;
    Ok(Redirect::to("/user/api-tokens"))
}

pub async fn delete_api_token(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    session: Session,
    Path(id): Path<i64>,
) -> Result<Redirect> {
    tokens::delete(&state.db, user.id, id).await?;
    auth::flash(&session, "API token deleted.").await?;
    Ok(Redirect::to("/user/api-tokens"))
}
