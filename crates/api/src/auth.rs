use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use buizz_core::api::{ApiError, LoginRequest, SESSION_COOKIE, SessionInfo};
use subtle::ConstantTimeEq;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::ApiConfig;
use crate::error::Failure;

const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const MAX_FAILURES: usize = 5;

struct Session {
    expires: Instant,
    alive: watch::Sender<()>,
}

#[derive(Clone)]
pub(crate) struct ActiveSession {
    revoked: watch::Receiver<()>,
    expires: Instant,
}

impl ActiveSession {
    pub(crate) async fn ended(mut self) {
        tokio::select! {
            _ = self.revoked.changed() => {}
            () = tokio::time::sleep_until(self.expires) => {}
        }
    }
}

enum Refused {
    WrongPassword,
    Locked { retry_after: Duration },
}

pub(crate) struct Auth {
    password: String,
    secure_cookies: bool,
    sessions: Mutex<HashMap<String, Session>>,
    failures: Mutex<VecDeque<Instant>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Auth {
    pub(crate) fn new(config: &ApiConfig) -> Self {
        Self {
            password: config.admin_password.clone(),
            secure_cookies: config.secure_cookies,
            sessions: Mutex::default(),
            failures: Mutex::default(),
        }
    }

    fn login(&self, password: &str) -> Result<String, Refused> {
        let now = Instant::now();
        {
            let mut failures = locked(&self.failures);
            while failures
                .front()
                .is_some_and(|failed| now.duration_since(*failed) >= FAILURE_WINDOW)
            {
                failures.pop_front();
            }
            if failures.len() >= MAX_FAILURES
                && let Some(oldest) = failures.front()
            {
                return Err(Refused::Locked {
                    retry_after: FAILURE_WINDOW.saturating_sub(now.duration_since(*oldest)),
                });
            }
            if !bool::from(password.as_bytes().ct_eq(self.password.as_bytes())) {
                failures.push_back(now);
                return Err(Refused::WrongPassword);
            }
        }
        let token = new_token();
        let (alive, _) = watch::channel(());
        let mut sessions = locked(&self.sessions);
        sessions.retain(|_, session| session.expires > now);
        sessions.insert(
            token.clone(),
            Session {
                expires: now + SESSION_TTL,
                alive,
            },
        );
        Ok(token)
    }

    pub(crate) fn authenticate(&self, token: &str) -> Option<ActiveSession> {
        let mut sessions = locked(&self.sessions);
        let session = sessions.get(token)?;
        if session.expires <= Instant::now() {
            sessions.remove(token);
            return None;
        }
        Some(ActiveSession {
            revoked: session.alive.subscribe(),
            expires: session.expires,
        })
    }

    fn revoke(&self, token: &str) {
        locked(&self.sessions).remove(token);
    }

    fn cookie(&self, value: String) -> Cookie<'static> {
        Cookie::build((SESSION_COOKIE, value))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Strict)
            .secure(self.secure_cookies)
            .build()
    }
}

pub(crate) async fn require_session(
    State(auth): State<Arc<Auth>>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    match jar
        .get(SESSION_COOKIE)
        .and_then(|cookie| auth.authenticate(cookie.value()))
    {
        Some(session) => {
            request.extensions_mut().insert(session);
            next.run(request).await
        }
        None => Failure::new(StatusCode::UNAUTHORIZED, "login required").into_response(),
    }
}

#[utoipa::path(post, path = "/api/auth/login", tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 204, description = "session cookie set", headers(("set-cookie" = String))),
        (status = 401, body = ApiError),
        (status = 429, description = "too many failed attempts", body = ApiError,
            headers(("retry-after" = u64)))))]
pub(crate) async fn login(
    State(auth): State<Arc<Auth>>,
    jar: CookieJar,
    Json(request): Json<LoginRequest>,
) -> Response {
    match auth.login(&request.password) {
        Ok(token) => (jar.add(auth.cookie(token)), StatusCode::NO_CONTENT).into_response(),
        Err(Refused::WrongPassword) => {
            Failure::new(StatusCode::UNAUTHORIZED, "wrong password").into_response()
        }
        Err(Refused::Locked { retry_after }) => {
            let seconds = retry_after.as_secs() + 1;
            let mut response = Failure::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too many failed attempts, try again later",
            )
            .into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
            response
        }
    }
}

#[utoipa::path(post, path = "/api/auth/logout", tag = "auth",
    responses((status = 204, description = "session cookie cleared")))]
pub(crate) async fn logout(State(auth): State<Arc<Auth>>, jar: CookieJar) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        auth.revoke(cookie.value());
    }
    let cleared = jar.remove(auth.cookie(String::new()));
    (cleared, StatusCode::NO_CONTENT).into_response()
}

#[utoipa::path(get, path = "/api/auth/session", tag = "auth",
    responses((status = 200, body = SessionInfo)))]
pub(crate) async fn session(State(auth): State<Arc<Auth>>, jar: CookieJar) -> Json<SessionInfo> {
    let authenticated = jar
        .get(SESSION_COOKIE)
        .is_some_and(|cookie| auth.authenticate(cookie.value()).is_some());
    Json(SessionInfo { authenticated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        Auth::new(&ApiConfig {
            admin_password: "secret".to_owned(),
            web_dir: None,
            secure_cookies: true,
        })
    }

    #[test]
    fn repeated_wrong_passwords_lock_out_even_the_right_one() {
        let auth = auth();
        for _ in 0..MAX_FAILURES {
            assert!(matches!(auth.login("guess"), Err(Refused::WrongPassword)));
        }
        assert!(matches!(auth.login("secret"), Err(Refused::Locked { .. })));
    }

    #[test]
    fn a_revoked_session_no_longer_authenticates() {
        let auth = auth();
        let token = auth
            .login("secret")
            .ok()
            .expect("the right password logs in");
        assert!(auth.authenticate(&token).is_some());
        auth.revoke(&token);
        assert!(auth.authenticate(&token).is_none());
    }
}
