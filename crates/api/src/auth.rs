use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
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
const GLOBAL_WINDOW: Duration = Duration::from_secs(15 * 60);
const GLOBAL_BUDGET: usize = 100;
const GLOBAL_PACE: Duration = Duration::from_secs(1);
const IPV6_PREFIX_BITS: u32 = 64;

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
    failures: Mutex<Failures>,
}

#[derive(Default)]
struct Failures {
    by_client: HashMap<IpAddr, VecDeque<Instant>>,
    global: VecDeque<Instant>,
    next_paced_check: Option<Instant>,
}

fn forget(history: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while history
        .front()
        .is_some_and(|failed| now.duration_since(*failed) >= window)
    {
        history.pop_front();
    }
}

impl Failures {
    fn forget_before(&mut self, now: Instant) {
        for history in self.by_client.values_mut() {
            forget(history, now, FAILURE_WINDOW);
        }
        self.by_client.retain(|_, history| !history.is_empty());
        forget(&mut self.global, now, GLOBAL_WINDOW);
    }

    fn admit(&mut self, client: IpAddr, now: Instant) -> Result<(), Refused> {
        if let Some(history) = self.by_client.get(&client)
            && history.len() >= MAX_FAILURES
            && let Some(oldest) = history.front()
        {
            return Err(Refused::Locked {
                retry_after: FAILURE_WINDOW.saturating_sub(now.duration_since(*oldest)),
            });
        }
        if self.global.len() < GLOBAL_BUDGET {
            return Ok(());
        }
        match self.next_paced_check {
            Some(next) if next > now => Err(Refused::Locked {
                retry_after: next - now,
            }),
            _ => {
                self.next_paced_check = Some(now + GLOBAL_PACE);
                Ok(())
            }
        }
    }

    fn record(&mut self, client: IpAddr, now: Instant) {
        self.by_client.entry(client).or_default().push_back(now);
        self.global.push_back(now);
    }
}

fn client_key(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from_bits(
                v6.to_bits() & (u128::MAX << (128 - IPV6_PREFIX_BITS)),
            )),
        },
    }
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

    fn login(&self, client: IpAddr, password: &str) -> Result<String, Refused> {
        let now = Instant::now();
        {
            let mut failures = locked(&self.failures);
            failures.forget_before(now);
            failures.admit(client_key(client), now)?;
            if !bool::from(password.as_bytes().ct_eq(self.password.as_bytes())) {
                failures.record(client_key(client), now);
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
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<LoginRequest>,
) -> Response {
    let peer = peer.map(|Extension(ConnectInfo(address))| address.ip());
    match auth.login(client_address(peer, &headers), &request.password) {
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

fn client_address(peer: Option<IpAddr>, headers: &HeaderMap) -> IpAddr {
    let peer = peer.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let behind_proxy = match peer {
        IpAddr::V4(address) => address.is_private() || address.is_loopback(),
        IpAddr::V6(address) => address.is_loopback() || address.is_unique_local(),
    };
    let forwarded = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
        .next_back();
    match forwarded {
        Some(client) if behind_proxy => client,
        _ => peer,
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
    fn wrong_passwords_lock_out_only_the_client_that_sent_them() {
        let auth = auth();
        let attacker = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
        let owner = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2));
        for _ in 0..MAX_FAILURES {
            assert!(matches!(
                auth.login(attacker, "guess"),
                Err(Refused::WrongPassword)
            ));
        }
        assert!(matches!(
            auth.login(attacker, "secret"),
            Err(Refused::Locked { .. })
        ));
        assert!(auth.login(owner, "secret").is_ok());
    }

    #[test]
    fn addresses_in_one_ipv6_slash_64_share_a_lockout() {
        let auth = auth();
        let network = 0x2001_0db8_0000_0001_u128 << 64;
        for host in 0..MAX_FAILURES {
            let address = IpAddr::V6(Ipv6Addr::from_bits(network | (host as u128 + 1)));
            assert!(matches!(
                auth.login(address, "guess"),
                Err(Refused::WrongPassword)
            ));
        }
        let fresh_host = IpAddr::V6(Ipv6Addr::from_bits(network | 0xffff));
        assert!(matches!(
            auth.login(fresh_host, "secret"),
            Err(Refused::Locked { .. })
        ));
    }

    #[test]
    fn many_addresses_guessing_slow_everyone_to_one_check_per_pace() {
        let auth = auth();
        for attacker in 0..GLOBAL_BUDGET {
            let address = IpAddr::V4(Ipv4Addr::from_bits(0x0a00_0000 + attacker as u32));
            assert!(matches!(
                auth.login(address, "guess"),
                Err(Refused::WrongPassword)
            ));
        }
        let owner = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2));
        let another = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 3));
        assert!(matches!(
            auth.login(another, "guess"),
            Err(Refused::WrongPassword)
        ));
        assert!(matches!(
            auth.login(owner, "secret"),
            Err(Refused::Locked { retry_after }) if retry_after <= GLOBAL_PACE
        ));
    }

    #[test]
    fn forwarded_addresses_count_only_behind_a_private_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.2.3.4, 203.0.113.7"),
        );
        let proxy = IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5));
        let direct = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2));
        assert_eq!(
            client_address(Some(proxy), &headers),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))
        );
        assert_eq!(client_address(Some(direct), &headers), direct);
    }

    #[test]
    fn a_revoked_session_no_longer_authenticates() {
        let auth = auth();
        let token = auth
            .login(IpAddr::V4(Ipv4Addr::LOCALHOST), "secret")
            .ok()
            .expect("the right password logs in");
        assert!(auth.authenticate(&token).is_some());
        auth.revoke(&token);
        assert!(auth.authenticate(&token).is_none());
    }
}
