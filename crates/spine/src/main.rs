mod config;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use buizz_api::{ApiConfig, AppState};
use buizz_core::run::{CostModel, RiskLimits};
use buizz_feed::{FeedConfig, FeedManager};
use buizz_ingest::{Ingest, IngestConfig};
use buizz_market::MarketStore;
use buizz_runner::{Runner, RunnerConfig, RunnerDeps};
use buizz_sources::Sources;
use buizz_sources::alpaca::{self, AlpacaClient};
use buizz_sources::alpaca_stream::SYMBOL_CAP;
use buizz_sources::edgar::EdgarClient;
use buizz_sources::tiingo::TiingoClient;
use buizz_sources::treasury::TreasuryClient;
use buizz_state::StateStore;
use buizz_supervisor::{Supervisor, SupervisorConfig};
use rust_decimal::Decimal;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

const MARKET_READERS: usize = 4;
const SNAPSHOTS_KEPT: usize = 14;
const START_CASH: Decimal = Decimal::from_parts(100_000, 0, 0, false, 0);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("cannot create {}", config.data_dir.display()))?;

    let market = MarketStore::open(
        &config.data_dir.join(buizz_market::FILE_NAME),
        MARKET_READERS,
    )
    .await?;
    if let Some(fixture_dir) = &config.fixture_dir {
        market.load_fixture(fixture_dir).await?;
    }
    let state = StateStore::open(&config.data_dir.join(buizz_state::FILE_NAME)).await?;

    let sources = Sources {
        alpaca: config.alpaca.clone().map(AlpacaClient::new).transpose()?,
        tiingo: config.tiingo.clone().map(TiingoClient::new).transpose()?,
        edgar: config.edgar.clone().map(EdgarClient::new).transpose()?,
        treasury: TreasuryClient::new(config.treasury.clone())?,
    };
    let feed = FeedManager::new(
        FeedConfig {
            cap: SYMBOL_CAP,
            stream: config.stream(),
        },
        market.clone(),
        sources.alpaca.clone(),
    );
    let ingest = Ingest::new(
        IngestConfig {
            tick: Duration::from_secs(60),
            snapshot_dir: config.data_dir.join("backups"),
            snapshot_keep: SNAPSHOTS_KEPT,
            history_start: alpaca::HISTORY_START,
            enabled: config.ingest,
        },
        market.clone(),
        state.clone(),
        sources,
    );
    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: config.spine_url(),
        max_parallel_backtests: std::thread::available_parallelism().map_or(1, usize::from),
        restart_delay: Duration::from_secs(1),
        kill_grace: Duration::from_secs(10),
    });
    let runner = Runner::new(
        RunnerConfig {
            bots_dir: config.bots_dir.clone(),
            start_cash: START_CASH,
            cost: CostModel::default(),
            risk: RiskLimits::default(),
        },
        RunnerDeps {
            market: market.clone(),
            state: state.clone(),
            supervisor,
            feed: feed.clone(),
            ingest: ingest.clone(),
        },
    );
    runner.recover().await?;

    let feed_task = feed.spawn();
    let ingest_task = ingest.spawn();

    let app = buizz_api::router(AppState {
        config: Arc::new(ApiConfig {
            admin_password: config.admin_password.clone(),
            web_dir: config.web_dir.clone(),
            secure_cookies: config.secure_cookies,
        }),
        runner: runner.clone(),
        market,
        state,
        feed,
        ingest,
    });
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("cannot bind {}", config.bind))?;
    tracing::info!(bind = %config.bind, "spine listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    runner.shutdown().await;
    feed_task.abort();
    if let Some(task) = ingest_task {
        task.abort();
    }
    Ok(())
}

async fn shutdown_signal() {
    let interrupt = tokio::signal::ctrl_c();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler installs");
    tokio::select! {
        _ = interrupt => {}
        _ = terminate.recv() => {}
    }
}
