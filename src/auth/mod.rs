//! Authentication: password hashing, sessions, invite tokens, CSRF, and login
//! rate limiting. Browser identity flows through an HTTP-only cookie holding a
//! random session token; the token is stored hashed in SQLite.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use chrono::{DateTime, Utc};
use rand::rngs::OsRng;

use crate::db::User;

pub const COOKIE_NAME: &str = "q_session";
const LOGIN_MAX_ATTEMPTS: usize = 10;
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("password hashing failed: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Per-IP login attempt limiter (in-memory; adequate for a personal tool).
#[derive(Default)]
pub struct LoginLimiter {
    inner: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns Ok if the IP may attempt a login, Err(403) if it is throttled.
    pub fn check(&self, ip: &str) -> Result<(), StatusCode> {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        let attempts = map.entry(ip.to_string()).or_default();
        attempts.retain(|t| now.duration_since(*t) < LOGIN_WINDOW);
        if attempts.len() >= LOGIN_MAX_ATTEMPTS {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        Ok(())
    }

    pub fn record_failure(&self, ip: &str) {
        let mut map = self.inner.lock().unwrap();
        map.entry(ip.to_string()).or_default().push(Instant::now());
    }

    pub fn record_success(&self, ip: &str) {
        self.inner.lock().unwrap().remove(ip);
    }
}

/// The authenticated user, attached to requests via the session cookie.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user: User,
    /// CSRF token bound to this session.
    pub csrf: String,
}

/// Rejection for missing/invalid authentication. Redirects browsers to /login.
pub enum AuthRejection {
    Unauthenticated,
    Internal(anyhow::Error),
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> axum::response::Response {
        match self {
            AuthRejection::Unauthenticated => Redirect::to("/login").into_response(),
            AuthRejection::Internal(e) => {
                tracing::error!("auth error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        }
    }
}

fn extract_cookie<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|pair| {
        let mut it = pair.trim().splitn(2, '=');
        let k = it.next()?;
        let v = it.next()?;
        if k == name {
            Some(v.to_string())
        } else {
            None
        }
    })
}

/// Extract the raw session token from request headers (for logout etc.).
pub fn token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    extract_cookie(headers, COOKIE_NAME)
}

#[async_trait]
impl FromRequestParts<crate::web::AppState> for CurrentUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::web::AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = extract_cookie(&parts.headers, COOKIE_NAME) else {
            return Err(AuthRejection::Unauthenticated);
        };
        let session = state
            .db
            .session_by_token(&token)
            .map_err(AuthRejection::Internal)?;
        let Some(session) = session else {
            return Err(AuthRejection::Unauthenticated);
        };
        if let Ok(exp) = DateTime::parse_from_rfc3339(&session.expires_at) {
            let exp = exp.with_timezone(&Utc);
            if exp < Utc::now() {
                return Err(AuthRejection::Unauthenticated);
            }
        }
        let Some(user) = state
            .db
            .user_by_id(&session.user_id)
            .map_err(AuthRejection::Internal)?
        else {
            return Err(AuthRejection::Unauthenticated);
        };
        Ok(CurrentUser {
            user,
            csrf: session.csrf,
        })
    }
}

/// Require the authenticated user to be an admin.
pub struct AdminUser(pub CurrentUser);

#[async_trait]
impl FromRequestParts<crate::web::AppState> for AdminUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::web::AppState,
    ) -> Result<Self, Self::Rejection> {
        let cur = CurrentUser::from_request_parts(parts, state).await?;
        if !cur.user.is_admin {
            return Err(AuthRejection::Unauthenticated);
        }
        Ok(AdminUser(cur))
    }
}

/// Verify a CSRF token posted by a form against the session's stored token.
pub fn check_csrf(user: &CurrentUser, posted: &str) -> bool {
    let a = user.csrf.as_bytes();
    let b = posted.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
