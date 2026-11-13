use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result};
use buizz_core::market::Symbol;
use buizz_sources::alpaca::{self, AlpacaConfig};
use buizz_sources::alpaca_stream::{self, STREAM_KEYS_ENV, StreamConfig, stream_keys};
use buizz_sources::anthropic::{self, AnthropicConfig};
use buizz_sources::cboe::{self, CboeConfig};
use buizz_sources::edgar::{self, EdgarConfig};
use buizz_sources::factset::{self, FactsetConfig};
use buizz_sources::fed::{self, FedConfig};
use buizz_sources::finra::{self, FinraConfig};
use buizz_sources::fred::{self, FredConfig};
use buizz_sources::ibkr::{self, IbkrConfig};
use buizz_sources::nasdaq::{self, NasdaqConfig};
use buizz_sources::ssga::{self, SsgaConfig};
use buizz_sources::tiingo::{self, TiingoConfig};
use buizz_sources::treasury::{self, TreasuryConfig};
use buizz_sources::wikipedia::{self, WikipediaConfig};
use chrono::TimeDelta;
use rust_decimal::Decimal;

pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub text_dirs: Vec<PathBuf>,
    pub text_fill_limit: f64,
    pub bots_dir: PathBuf,
    pub web_dir: Option<PathBuf>,
    pub fixture_dir: Option<PathBuf>,
    pub admin_username: String,
    pub admin_password: String,
    pub secure_cookies: bool,
    pub ingest: bool,
    pub alpaca: Option<AlpacaConfig>,
    pub streams: Vec<StreamConfig>,
    pub news_url: String,
    pub tiingo: Option<TiingoConfig>,
    pub edgar: Option<EdgarConfig>,
    pub fred: Option<FredConfig>,
    pub treasury: TreasuryConfig,
    pub wikipedia: Option<WikipediaConfig>,
    pub ibkr: IbkrConfig,
    pub nasdaq: NasdaqConfig,
    pub cboe: CboeConfig,
    pub fed: FedConfig,
    pub factset: Option<FactsetConfig>,
    pub finra: FinraConfig,
    pub ssga: SsgaConfig,
    pub anthropic: Option<AnthropicConfig>,
    pub ingest_symbols: Option<BTreeSet<Symbol>>,
    pub duckdb_memory: String,
    pub duckdb_threads: usize,
    pub live_slots: usize,
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

fn fill_limit(value: Option<String>) -> Result<f64> {
    value.map_or(Ok(buizz_market::FILL_LIMIT), |share| {
        share
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|share| *share > 0.0 && *share <= 1.0)
            .context("BUIZZ_TEXT_FILL_LIMIT must be a share of the volume above 0 and at most 1")
    })
}

fn spend_cap(value: Option<String>) -> Result<Decimal> {
    value.map_or(Ok(anthropic::DEFAULT_SPEND_CAP), |dollars| {
        dollars
            .trim()
            .parse::<Decimal>()
            .ok()
            .filter(|cap| !cap.is_sign_negative())
            .with_context(|| format!("{} must be a dollar amount", anthropic::SPEND_CAP_ENV))
    })
}

pub fn clock_offset() -> Result<TimeDelta> {
    var("BUIZZ_CLOCK_OFFSET_SECONDS").map_or(Ok(TimeDelta::zero()), |seconds| {
        seconds
            .parse()
            .ok()
            .and_then(TimeDelta::try_seconds)
            .context("BUIZZ_CLOCK_OFFSET_SECONDS must be a whole number of seconds")
    })
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
        let stream_accounts = match var(STREAM_KEYS_ENV) {
            Some(keys) => stream_keys(&keys)?,
            None => alpaca.iter().cloned().collect(),
        };
        Ok(Self {
            bind,
            data_dir: data_dir(),
            text_dirs: var("BUIZZ_TEXT_DIRS")
                .map(|dirs| {
                    dirs.split(',')
                        .map(str::trim)
                        .filter(|dir| !dir.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            text_fill_limit: fill_limit(var("BUIZZ_TEXT_FILL_LIMIT"))?,
            bots_dir: var_or("BUIZZ_BOTS_DIR", "bots").into(),
            web_dir: var("BUIZZ_WEB_DIR").map(PathBuf::from),
            fixture_dir: var("BUIZZ_FIXTURE_DIR").map(PathBuf::from),
            admin_username: var_or("BUIZZ_ADMIN_USERNAME", "admin"),
            admin_password: admin_password(var("BUIZZ_ADMIN_PASSWORD"))?,
            secure_cookies: flag("BUIZZ_SECURE_COOKIES", true)?,
            ingest: flag("BUIZZ_INGEST", true)?,
            alpaca,
            streams: stream_accounts.into_iter().map(StreamConfig::new).collect(),
            news_url: var_or("ALPACA_NEWS_URL", alpaca_stream::NEWS_URL),
            tiingo: var("TIINGO_API_KEY").map(|api_key| TiingoConfig {
                base_url: var_or("TIINGO_URL", tiingo::BASE_URL),
                ..TiingoConfig::new(api_key)
            }),
            wikipedia: var("SEC_USER_AGENT").map(|contact| WikipediaConfig {
                user_agent: format!("buizz/{} ({contact})", env!("CARGO_PKG_VERSION")),
                base_url: var_or("WIKIPEDIA_URL", wikipedia::BASE_URL),
                rest_url: var_or(wikipedia::REST_URL_ENV, wikipedia::REST_URL),
            }),
            factset: var("SEC_USER_AGENT").map(|contact| FactsetConfig {
                user_agent: format!("buizz/{} ({contact})", env!("CARGO_PKG_VERSION")),
                base_url: var_or(factset::URL_ENV, factset::BASE_URL),
                archive_url: var_or(factset::ARCHIVE_URL_ENV, factset::ARCHIVE_URL),
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
            cboe: CboeConfig {
                base_url: var_or(cboe::URL_ENV, cboe::BASE_URL),
            },
            fed: FedConfig {
                base_url: var_or(fed::URL_ENV, fed::BASE_URL),
            },
            ssga: SsgaConfig {
                base_url: var_or(ssga::URL_ENV, ssga::BASE_URL),
            },
            finra: FinraConfig {
                api_url: var_or(finra::API_URL_ENV, finra::API_URL),
                cdn_url: var_or(finra::CDN_URL_ENV, finra::CDN_URL),
                site_url: var_or(finra::SITE_URL_ENV, finra::SITE_URL),
            },
            anthropic: var(anthropic::KEY_ENV)
                .map(|api_key| -> Result<AnthropicConfig> {
                    Ok(AnthropicConfig {
                        base_url: var_or(anthropic::URL_ENV, anthropic::BASE_URL),
                        spend_cap: spend_cap(var(anthropic::SPEND_CAP_ENV))?,
                        ..AnthropicConfig::new(api_key)
                    })
                })
                .transpose()?,
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
            live_slots: var_or("BUIZZ_LIVE_SLOTS", "2")
                .parse()
                .context("BUIZZ_LIVE_SLOTS must be a whole number")?,
        })
    }

    pub fn news_stream(&self) -> Option<StreamConfig> {
        self.alpaca.clone().map(|alpaca| StreamConfig {
            url: self.news_url.clone(),
            ..StreamConfig::new(alpaca)
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
