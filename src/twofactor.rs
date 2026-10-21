//! TOTP compatible with authenticator apps: SHA1, 6 digits, 30 second steps, one step of skew.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rand::distr::{Alphanumeric, SampleString};
use sqlx::SqlitePool;
use totp_rs::{Algorithm, Builder, Secret, Totp};

use crate::auth::User;

const ISSUER: &str = "CM Provisioning";
const RECOVERY_CODES: usize = 8;

fn totp(secret: &str, account: &str) -> Option<Totp> {
    Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(Secret::try_from_base32(secret).ok()?)
        .with_issuer(Some(ISSUER))
        .with_account_name(account)
        .build()
        .ok()
}

pub fn generate_secret() -> String {
    Secret::generate().to_base32()
}

/// The time step a valid code belongs to.
pub fn verify_step(secret: &str, account: &str, code: &str) -> Option<u64> {
    totp(secret, account)?.check_current(code)
}

pub fn verify(secret: &str, account: &str, code: &str) -> bool {
    verify_step(secret, account, code).is_some()
}

/// The last accepted time step per user, so an observed code cannot be replayed within its window.
#[derive(Clone, Default)]
pub struct UsedSteps(Arc<Mutex<HashMap<i64, u64>>>);

impl UsedSteps {
    pub fn accept(&self, user_id: i64, step: u64) -> bool {
        let mut used = self.0.lock().unwrap();
        match used.get(&user_id) {
            Some(&last) if step <= last => false,
            _ => {
                used.insert(user_id, step);
                true
            }
        }
    }
}

/// A PNG data URI of the otpauth:// URL for scanning into an authenticator app.
pub fn qr_code(secret: &str, account: &str) -> Option<String> {
    let png = totp(secret, account)?.to_qr_base64().ok()?;
    Some(format!("data:image/png;base64,{png}"))
}

/// Same shape as Fortify's codes.
pub fn generate_recovery_codes() -> Vec<String> {
    let mut rng = rand::rng();
    (0..RECOVERY_CODES)
        .map(|_| format!("{}-{}", Alphanumeric.sample_string(&mut rng, 10), Alphanumeric.sample_string(&mut rng, 10)))
        .collect()
}

pub fn recovery_codes(user: &User) -> Vec<String> {
    user.two_factor_recovery_codes
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

pub async fn enable(db: &SqlitePool, user_id: i64, secret: &str, codes: &[String]) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET two_factor_secret = ?, two_factor_recovery_codes = ? WHERE id = ?")
        .bind(secret)
        .bind(serde_json::to_string(codes).unwrap())
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn replace_recovery_codes(db: &SqlitePool, user_id: i64, codes: &[String]) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET two_factor_recovery_codes = ? WHERE id = ?")
        .bind(serde_json::to_string(codes).unwrap())
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn disable(db: &SqlitePool, user_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET two_factor_secret = NULL, two_factor_recovery_codes = NULL WHERE id = ?")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Recovery codes are single use.
pub async fn consume_recovery_code(db: &SqlitePool, user: &User, code: &str) -> sqlx::Result<bool> {
    let mut codes = recovery_codes(user);
    let Some(index) = codes.iter().position(|c| c == code) else {
        return Ok(false);
    };
    codes.remove(index);
    replace_recovery_codes(db, user.id, &codes).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_current_code() {
        let secret = generate_secret();
        let code = totp(&secret, "a@example.com").unwrap().generate_current().to_string();
        assert!(verify(&secret, "a@example.com", &code));
        assert!(!verify(&secret, "a@example.com", "abcdef"));
    }

    #[test]
    fn refuses_replayed_steps() {
        let used = UsedSteps::default();
        assert!(used.accept(1, 100));
        assert!(!used.accept(1, 100));
        assert!(!used.accept(1, 99));
        assert!(used.accept(2, 100));
        assert!(used.accept(1, 101));
    }

    #[test]
    fn recovery_codes_have_fortify_shape() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), 8);
        assert!(codes.iter().all(|c| c.len() == 21 && c.as_bytes()[10] == b'-'));
    }

    #[test]
    fn qr_code_is_png_data_uri() {
        assert!(qr_code(&generate_secret(), "a@example.com").unwrap().starts_with("data:image/png;base64,"));
    }
}
