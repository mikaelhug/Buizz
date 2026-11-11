use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use buizz_sources::alpaca::{self, AlpacaConfig};
use buizz_sources::alpaca_stream::{self, StreamConfig};
use buizz_sources::edgar::{self, EdgarConfig};
use buizz_sources::tiingo::{self, TiingoConfig};
use buizz_sources::treasury::{self, TreasuryConfig};

pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub bots_dir: PathBuf,
    pub web_dir: Option<PathBuf>,
    pub fixture_dir: Option<PathBuf>,
    pub admin_password: String,
    pub secure_cookies: bool,
    pub ingest: bool,
    pub alpaca: Option<AlpacaConfig>,
    pub stream_url: String,
    pub tiingo: Option<TiingoConfig>,
    pub edgar: Option<EdgarConfig>,
    pub treasury: TreasuryConfig,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn var_or(name: &str, default: &str) -> String {
    var(name).unwrap_or_else(|| default.to_owned())
}

fn flag(name: &str, default: bool) -> Result<bool> {
    match var(name).as_deref() {
        None => Ok(default),
        Some("1" | "true" | "on") => Ok(true),
        Some("0" | "false" | "off") => Ok(false),
        Some(other) => anyhow::bail!("{name} must be true or false, got {other:?}"),
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind = var_or("BUIZZ_BIND", "0.0.0.0:8080")
            .parse()
            .context("BUIZZ_BIND must be host:port")?;
        let alpaca = match (var("ALPACA_KEY_ID"), var("ALPACA_SECRET_KEY")) {
            (Some(key_id), Some(secret_key)) => Some(AlpacaConfig {
                data_url: var_or("ALPACA_DATA_URL", alpaca::DATA_URL),
                trading_url: var_or("ALPACA_TRADING_URL", alpaca::TRADING_URL),
                ..AlpacaConfig::new(key_id, secret_key)
            }),
            _ => None,
        };
        Ok(Self {
            bind,
            data_dir: var_or("BUIZZ_DATA_DIR", "data").into(),
            bots_dir: var_or("BUIZZ_BOTS_DIR", "bots").into(),
            web_dir: var("BUIZZ_WEB_DIR").map(PathBuf::from),
            fixture_dir: var("BUIZZ_FIXTURE_DIR").map(PathBuf::from),
            admin_password: var("BUIZZ_ADMIN_PASSWORD")
                .context("BUIZZ_ADMIN_PASSWORD is required")?,
            secure_cookies: flag("BUIZZ_SECURE_COOKIES", true)?,
            ingest: flag("BUIZZ_INGEST", true)?,
            alpaca,
            stream_url: var_or("ALPACA_STREAM_URL", alpaca_stream::STREAM_URL),
            tiingo: var("TIINGO_API_KEY").map(|api_key| TiingoConfig {
                base_url: var_or("TIINGO_URL", tiingo::BASE_URL),
                ..TiingoConfig::new(api_key)
            }),
            edgar: var("SEC_USER_AGENT").map(|user_agent| EdgarConfig {
                data_url: var_or("SEC_DATA_URL", edgar::DATA_URL),
                www_url: var_or("SEC_WWW_URL", edgar::WWW_URL),
                ..EdgarConfig::new(user_agent)
            }),
            treasury: TreasuryConfig {
                base_url: var_or("TREASURY_URL", treasury::BASE_URL),
            },
        })
    }

    pub fn spine_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.bind.port())
    }

    pub fn stream(&self) -> Option<StreamConfig> {
        self.alpaca.clone().map(|alpaca| StreamConfig {
            alpaca,
            url: self.stream_url.clone(),
            reconnect_initial: std::time::Duration::from_secs(1),
            reconnect_max: std::time::Duration::from_secs(60),
        })
    }
}
