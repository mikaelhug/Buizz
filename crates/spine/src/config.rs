use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result};
use buizz_core::market::Symbol;
use buizz_sources::alpaca::{self, AlpacaConfig};
use buizz_sources::alpaca_stream::{self, StreamConfig};
use buizz_sources::edgar::{self, EdgarConfig};
use buizz_sources::fred::{self, FredConfig};
use buizz_sources::ibkr::{self, IbkrConfig};
use buizz_sources::nasdaq::{self, NasdaqConfig};
use buizz_sources::tiingo::{self, TiingoConfig};
use buizz_sources::treasury::{self, TreasuryConfig};
use buizz_sources::wikipedia::{self, WikipediaConfig};

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
    pub fred: Option<FredConfig>,
    pub treasury: TreasuryConfig,
    pub wikipedia: Option<WikipediaConfig>,
    pub ibkr: IbkrConfig,
    pub nasdaq: NasdaqConfig,
    pub ingest_symbols: Option<BTreeSet<Symbol>>,
    pub duckdb_memory: String,
    pub duckdb_threads: usize,
}

const MIN_ADMIN_PASSWORD_CHARS: usize = 12;

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

fn admin_password(value: Option<String>) -> Result<String> {
    let password = value.context("BUIZZ_ADMIN_PASSWORD is required")?;
    let length = password.chars().count();
    anyhow::ensure!(
        length >= MIN_ADMIN_PASSWORD_CHARS,
        "BUIZZ_ADMIN_PASSWORD must be at least {MIN_ADMIN_PASSWORD_CHARS} characters, it has {length}"
    );
    Ok(password)
}

pub fn data_dir() -> PathBuf {
    var_or("BUIZZ_DATA_DIR", "data").into()
}

pub fn spine_url(bound: SocketAddr) -> String {
    let ip = match bound.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("ws://{}", SocketAddr::new(ip, bound.port()))
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
            data_dir: data_dir(),
            bots_dir: var_or("BUIZZ_BOTS_DIR", "bots").into(),
            web_dir: var("BUIZZ_WEB_DIR").map(PathBuf::from),
            fixture_dir: var("BUIZZ_FIXTURE_DIR").map(PathBuf::from),
            admin_password: admin_password(var("BUIZZ_ADMIN_PASSWORD"))?,
            secure_cookies: flag("BUIZZ_SECURE_COOKIES", true)?,
            ingest: flag("BUIZZ_INGEST", true)?,
            alpaca,
            stream_url: var_or("ALPACA_STREAM_URL", alpaca_stream::STREAM_URL),
            tiingo: var("TIINGO_API_KEY").map(|api_key| TiingoConfig {
                base_url: var_or("TIINGO_URL", tiingo::BASE_URL),
                ..TiingoConfig::new(api_key)
            }),
            wikipedia: var("SEC_USER_AGENT").map(|contact| WikipediaConfig {
                user_agent: format!("buizz/{} ({contact})", env!("CARGO_PKG_VERSION")),
                base_url: var_or("WIKIPEDIA_URL", wikipedia::BASE_URL),
            }),
            edgar: var("SEC_USER_AGENT").map(|user_agent| EdgarConfig {
                data_url: var_or("SEC_DATA_URL", edgar::DATA_URL),
                www_url: var_or("SEC_WWW_URL", edgar::WWW_URL),
                ..EdgarConfig::new(user_agent)
            }),
            ibkr: IbkrConfig {
                address: var_or("IBKR_FTP_ADDRESS", ibkr::ADDRESS),
            },
            fred: var(fred::KEY_ENV).map(|api_key| FredConfig {
                base_url: var_or(fred::URL_ENV, fred::BASE_URL),
                ..FredConfig::new(api_key)
            }),
            treasury: TreasuryConfig {
                base_url: var_or("TREASURY_URL", treasury::BASE_URL),
            },
            nasdaq: NasdaqConfig {
                base_url: var_or(nasdaq::URL_ENV, nasdaq::BASE_URL),
            },
            ingest_symbols: var("BUIZZ_INGEST_SYMBOLS").map(|symbols| {
                symbols
                    .split(',')
                    .map(str::trim)
                    .filter(|symbol| !symbol.is_empty())
                    .map(Symbol::new)
                    .collect()
            }),
            duckdb_memory: var_or("BUIZZ_DUCKDB_MEMORY", "1GB"),
            duckdb_threads: var_or("BUIZZ_DUCKDB_THREADS", "2")
                .parse()
                .context("BUIZZ_DUCKDB_THREADS must be a whole number")?,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_admin_password_is_refused() {
        assert!(admin_password(Some("eleven-char".to_owned())).is_err());
        assert!(admin_password(Some("twelve-chars".to_owned())).is_ok());
    }
}
