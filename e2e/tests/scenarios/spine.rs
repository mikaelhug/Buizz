use std::future::Future;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use buizz_core::api::{JobView, RunSummary, SpineEvent};
use buizz_core::run::{RunEvent, RunEventKind, RunId};
use reqwest::{Client, StatusCode, header};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep, timeout};

use crate::fake::FakeProviders;
use crate::world::repository;

pub const PASSWORD: &str = "e2e-password";
const DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(100);
const EVENTS_PAGE: u32 = 1000;

pub async fn eventually<T, F: Future<Output = Option<T>>>(
    what: &str,
    mut probe: impl FnMut() -> F,
) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = probe().await {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(POLL).await;
    }
}

pub struct Options {
    pub bots: &'static [&'static str],
    pub tiingo: bool,
}

pub struct Spine {
    pub base: String,
    pub fake: FakeProviders,
    options: Options,
    port: u16,
    dir: TempDir,
    child: Option<Child>,
    client: Client,
    cookie: String,
}

fn binary() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("the target directory")
        .join("debug/buizz-spine");
    assert!(
        path.is_file(),
        "{} is missing; run the scenarios with `just e2e`",
        path.display()
    );
    path
}

fn bot_dir(name: &str) -> PathBuf {
    let example = repository().join("bots").join(name);
    if example.is_dir() {
        example
    } else {
        repository().join("e2e/bots").join(name)
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

impl Spine {
    pub async fn start(fake: FakeProviders, options: Options) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("buizz-e2e-")
            .tempdir()
            .expect("a working directory");
        let bots = dir.path().join("bots");
        std::fs::create_dir(&bots).expect("the bots directory");
        for name in options.bots {
            std::os::unix::fs::symlink(bot_dir(name), bots.join(name)).expect("bot link");
        }
        let mut spine = Self {
            base: String::new(),
            port: free_port(),
            fake,
            options,
            dir,
            child: None,
            client: Client::new(),
            cookie: String::new(),
        };
        spine.base = format!("http://127.0.0.1:{}", spine.port);
        spine.launch().await;
        spine
    }

    async fn launch(&mut self) {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.path().join("spine.log"))
            .expect("the spine log");
        let url = &self.fake.url;
        let mut command = Command::new(binary());
        command
            .current_dir(self.dir.path())
            .env_clear()
            .envs(
                ["PATH", "HOME", "TMPDIR"]
                    .into_iter()
                    .filter_map(|name| std::env::var_os(name).map(|value| (name, value))),
            )
            .env("RUST_LOG", "info")
            .env("BUIZZ_BIND", format!("127.0.0.1:{}", self.port))
            .env("BUIZZ_ADMIN_PASSWORD", PASSWORD)
            .env("BUIZZ_SECURE_COOKIES", "false")
            .env("BUIZZ_WEB_DIR", repository().join("web/dist"))
            .env("ALPACA_KEY_ID", "e2e-key")
            .env("ALPACA_SECRET_KEY", "e2e-secret")
            .env("ALPACA_DATA_URL", url)
            .env("ALPACA_TRADING_URL", url)
            .env("ALPACA_STREAM_URL", self.fake.stream_url())
            .env("SEC_USER_AGENT", "buizz-e2e e2e@example.test")
            .env("SEC_DATA_URL", url)
            .env("SEC_WWW_URL", url)
            .env("TREASURY_URL", url)
            .stdout(log.try_clone().expect("the spine log"))
            .stderr(log)
            .kill_on_drop(true);
        if self.options.tiingo {
            command
                .env("TIINGO_API_KEY", "e2e-tiingo")
                .env("TIINGO_URL", url);
        }
        self.child = Some(command.spawn().expect("the spine starts"));
        let session = format!("{}/api/auth/session", self.base);
        eventually("the spine to listen", || async {
            self.client.get(&session).send().await.ok()
        })
        .await;
        let login = self.login(PASSWORD).await;
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        let cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("login sets a session cookie");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Strict"), "{cookie}");
        self.cookie = cookie
            .split(';')
            .next()
            .expect("the cookie has a value")
            .to_owned();
    }

    pub async fn login(&self, password: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/api/auth/login", self.base))
            .json(&json!({ "password": password }))
            .send()
            .await
            .expect("login is answered")
    }

    pub async fn anonymous(&self, path: &str) -> StatusCode {
        self.client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .expect("the spine answers")
            .status()
    }

    pub async fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().await.expect("the spine is killed");
        }
    }

    pub async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = nix::unistd::Pid::from_raw(
                i32::try_from(child.id().expect("a running spine")).expect("pid fits"),
            );
            nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM)
                .expect("the spine is signalled");
            timeout(DEADLINE, child.wait())
                .await
                .expect("the spine stops in time")
                .expect("the spine exits");
        }
    }

    pub async fn restart(&mut self) {
        self.launch().await;
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("spine.log")).unwrap_or_default()
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, String) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header(header::COOKIE, &self.cookie);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("the spine answers");
        let status = response.status();
        (status, response.text().await.expect("a response body"))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> T {
        let (status, body) = self.request(reqwest::Method::GET, path, None).await;
        assert!(status.is_success(), "GET {path}: {status} {body}");
        serde_json::from_str(&body).unwrap_or_else(|error| panic!("GET {path}: {error}: {body}"))
    }

    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: Value) -> T {
        let (status, text) = self.request(reqwest::Method::POST, path, Some(body)).await;
        assert!(status.is_success(), "POST {path}: {status} {text}");
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("POST {path}: {error}: {text}"))
    }

    pub async fn ingested(&self) -> Vec<JobView> {
        eventually("the first ingest tick", || async {
            let jobs: Vec<JobView> = self.get("/api/data/jobs").await;
            jobs.iter()
                .all(|job| job.last_finished.is_some())
                .then_some(jobs)
        })
        .await
    }

    pub async fn run(&self, id: RunId) -> RunSummary {
        self.get(&format!("/api/runs/{id}")).await
    }

    pub async fn finished(&self, id: RunId) -> RunSummary {
        let run = eventually("the run to finish", || async {
            let run = self.run(id).await;
            run.status.is_finished().then_some(run)
        })
        .await;
        assert_eq!(
            run.status,
            buizz_core::run::RunStatus::Completed,
            "{:?}\n{}",
            run.message,
            self.log()
        );
        run
    }

    pub async fn events(&self, id: RunId) -> Vec<RunEvent> {
        let mut events: Vec<RunEvent> = Vec::new();
        loop {
            let after = events
                .last()
                .map(|event| format!("&after={}", event.seq))
                .unwrap_or_default();
            let page: buizz_core::api::EventsPage = self
                .get(&format!("/api/runs/{id}/events?limit={EVENTS_PAGE}{after}"))
                .await;
            let more = page.next_after.is_some();
            events.extend(page.items);
            if !more {
                return events;
            }
        }
    }

    pub async fn logs(&self, id: RunId) -> Vec<(Option<buizz_core::time::Timestamp>, String)> {
        self.events(id)
            .await
            .into_iter()
            .filter_map(|event| match event.kind {
                RunEventKind::Log { message, .. } => Some((event.clock, message)),
                _ => None,
            })
            .collect()
    }

    pub async fn orders(&self, id: RunId) -> Vec<buizz_core::api::OrderView> {
        let page: buizz_core::api::OrdersPage =
            self.get(&format!("/api/runs/{id}/orders?limit=1000")).await;
        assert!(page.next_after.is_none(), "more than one page of orders");
        page.items
    }

    pub async fn subscribe(&self) -> EventStream {
        let response = self
            .client
            .get(format!("{}/api/events", self.base))
            .header(header::COOKIE, &self.cookie)
            .send()
            .await
            .expect("the event stream opens");
        assert_eq!(response.status(), StatusCode::OK);
        EventStream {
            response,
            buffer: String::new(),
        }
    }

    pub async fn logout(&self) {
        let (status, _) = self
            .request(reqwest::Method::POST, "/api/auth/logout", None)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = self.request(reqwest::Method::GET, "/api/bots", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

pub struct EventStream {
    response: reqwest::Response,
    buffer: String,
}

impl EventStream {
    pub async fn next(&mut self) -> SpineEvent {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let frame: String = self.buffer.drain(..end + 2).collect();
                if let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) {
                    return serde_json::from_str(data).expect("events are spine events");
                }
                continue;
            }
            let chunk = timeout(DEADLINE, self.response.chunk())
                .await
                .expect("events arrive in time")
                .expect("the stream reads")
                .expect("the stream stays open");
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
}
