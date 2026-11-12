mod config;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use buizz_api::{ApiConfig, AppState};
use buizz_core::time::Timestamp;
use buizz_feed::{FeedConfig, FeedManager};
use buizz_ingest::{Ingest, IngestConfig};
use buizz_market::MarketStore;
use buizz_runner::{Runner, RunnerConfig, RunnerDeps};
use buizz_sources::Sources;
use buizz_sources::alpaca::AlpacaClient;
use buizz_sources::alpaca_stream::SYMBOL_CAP;
use buizz_sources::cboe::CboeClient;
use buizz_sources::edgar::EdgarClient;
use buizz_sources::fed::FedClient;
use buizz_sources::fred::FredClient;
use buizz_sources::ibkr::IbkrClient;
use buizz_sources::nasdaq::NasdaqClient;
use buizz_sources::tiingo::TiingoClient;
use buizz_sources::treasury::TreasuryClient;
use buizz_sources::wikipedia::WikipediaClient;
use buizz_state::StateStore;
use buizz_supervisor::{Supervisor, SupervisorConfig};
use rust_decimal::Decimal;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

const MARKET_READERS: usize = 4;
const SNAPSHOTS_KEPT: usize = 14;
const INGEST_STOP_GRACE: Duration = Duration::from_secs(20);
const START_CASH: Decimal = Decimal::from_parts(100_000, 0, 0, false, 0);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    buizz_core::time::offset_clock(config::clock_offset()?);
    let mut arguments = std::env::args().skip(1);
    match (
        arguments.next().as_deref(),
        arguments.next(),
        arguments.next(),
    ) {
        (None, ..) => serve().await,
        (Some("restore"), Some(backup), None) => restore(backup.into()).await,
        _ => anyhow::bail!("usage: buizz-spine [restore <market backup directory>]"),
    }
}

async fn restore(backup: PathBuf) -> Result<()> {
    let data_dir = config::data_dir();
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("cannot create {}", data_dir.display()))?;
    let market = MarketStore::open(&data_dir.join(buizz_market::FILE_NAME), 1).await?;
    let state = StateStore::open(&data_dir.join(buizz_state::FILE_NAME)).await?;
    let restored = buizz_ingest::restore(&market, &state, backup.clone(), Timestamp::now()).await?;
    tracing::info!(
        backup = %backup.display(),
        parts = restored.parts,
        batches = restored.batches,
        rows = restored.rows,
        "restored the market backup; ingest fetches everything else again on the next start"
    );
    Ok(())
}

async fn serve() -> Result<()> {
    let config = Config::from_env()?;
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("cannot bind {}", config.bind))?;
    let bound = listener
        .local_addr()
        .context("cannot read the bound address")?;
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("cannot create {}", config.data_dir.display()))?;

    let market = MarketStore::open(
        &config.data_dir.join(buizz_market::FILE_NAME),
        MARKET_READERS,
    )
    .await?;
    market
        .limit(config.duckdb_memory.clone(), config.duckdb_threads)
        .await?;
    let market_was_empty = market.is_empty().await?;
    if let Some(fixture_dir) = &config.fixture_dir {
        market.load_fixture(fixture_dir).await?;
    }
    let state = StateStore::open(&config.data_dir.join(buizz_state::FILE_NAME)).await?;
    if market_was_empty && state.reset_market(Timestamp::now()).await? {
        tracing::warn!(
            "the market store is new but the state store has ingest history; that history \
             no longer describes the market store, so ingest starts over"
        );
    }

    let sources = Sources {
        alpaca: config.alpaca.clone().map(AlpacaClient::new).transpose()?,
        tiingo: config.tiingo.clone().map(TiingoClient::new).transpose()?,
        edgar: config.edgar.clone().map(EdgarClient::new).transpose()?,
        fred: config.fred.clone().map(FredClient::new).transpose()?,
        treasury: TreasuryClient::new(config.treasury.clone())?,
        wikipedia: config
            .wikipedia
            .clone()
            .map(WikipediaClient::new)
            .transpose()?,
        ibkr: Some(IbkrClient::new(config.ibkr.clone())),
        nasdaq: Some(NasdaqClient::new(config.nasdaq.clone())?),
        cboe: Some(CboeClient::new(config.cboe.clone())?),
        fed: Some(FedClient::new(config.fed.clone())?),
    };
    let feed = FeedManager::new(
        FeedConfig {
            symbols_per_connection: SYMBOL_CAP,
            streams: config.streams.clone(),
        },
        market.clone(),
        sources.alpaca.clone(),
    );
    let ingest = Ingest::new(
        IngestConfig {
            tick: Duration::from_secs(60),
            snapshot_dir: config.data_dir.join("backups"),
            snapshot_keep: SNAPSHOTS_KEPT,
            history_start: buizz_core::market::HISTORY_START,
            enabled: config.ingest,
            only: config.ingest_symbols.clone(),
        },
        market.clone(),
        state.clone(),
        sources,
    );
    let supervisor = Supervisor::new(SupervisorConfig {
        spine_url: config::spine_url(bound),
        log_dir: config.data_dir.join("logs"),
        max_parallel_backtests: std::thread::available_parallelism().map_or(1, usize::from),
        restart_delay: Duration::from_secs(1),
        kill_grace: Duration::from_secs(10),
    });
    let runner = Runner::new(
        RunnerConfig {
            bots_dir: config.bots_dir.clone(),
            start_cash: START_CASH,
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
    let news_task = ingest.record_news(config.news_stream());

    let (shutting_down, shutdown) = watch::channel(false);
    let app = buizz_api::router(AppState {
        config: Arc::new(ApiConfig {
            admin_password: config.admin_password.clone(),
            web_dir: config.web_dir.clone(),
            secure_cookies: config.secure_cookies,
            spine_ip: bound.ip(),
        }),
        runner: runner.clone(),
        market: market.clone(),
        state,
        feed,
        ingest: ingest.clone(),
        shutdown,
    });
    tracing::info!(addr = %bound, "spine listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutting_down))
    .await?;

    ingest.stop(INGEST_STOP_GRACE).await;
    for task in [ingest_task, news_task].into_iter().flatten() {
        task.abort();
    }
    runner.shutdown().await;
    feed_task.abort();
    market.checkpoint().await?;
    Ok(())
}

async fn shutdown_signal(shutting_down: watch::Sender<bool>) {
    let mut interrupt = signal(SignalKind::interrupt()).expect("SIGINT handler installs");
    let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM handler installs");
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down; a second signal exits at once");
    shutting_down.send_replace(true);
    tokio::spawn(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
        tracing::warn!("second signal, exiting without a clean shutdown");
        std::process::exit(1);
    });
}
