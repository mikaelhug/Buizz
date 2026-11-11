use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

pub use buizz_core::market::SIP_DELAY;
use buizz_core::market::{
    Bar, Calendar, CorporateAction, CorporateActionKind, Feed, Instrument, Session, Symbol,
    Timeframe,
};
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;

use crate::http::{HttpClient, REDACTED, RateLimit, SourceError};

mod wire;

use wire::{ActionsPage, BarsPage, Paged, WireAsset, WireCalendarDay};

pub const DATA_URL: &str = "https://data.alpaca.markets";
pub const TRADING_URL: &str = "https://paper-api.alpaca.markets";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 200,
    per: Duration::from_secs(60),
};
pub const HISTORY_START: NaiveDate = NaiveDate::from_ymd_opt(2016, 1, 1).expect("valid date");
pub const MAX_PAGE_LIMIT: u32 = 10_000;
pub const ACTIONS_PAGE_LIMIT: u32 = 1_000;
const ACTIONS_SYMBOLS_PER_REQUEST: usize = 100;

#[derive(Clone, PartialEq, Eq)]
pub struct AlpacaConfig {
    pub key_id: String,
    pub secret_key: String,
    pub data_url: String,
    pub trading_url: String,
}

impl AlpacaConfig {
    pub fn new(key_id: String, secret_key: String) -> Self {
        Self {
            key_id,
            secret_key,
            data_url: DATA_URL.to_owned(),
            trading_url: TRADING_URL.to_owned(),
        }
    }
}

impl fmt::Debug for AlpacaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlpacaConfig")
            .field("key_id", &self.key_id)
            .field("secret_key", &REDACTED)
            .field("data_url", &self.data_url)
            .field("trading_url", &self.trading_url)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarsRequest {
    pub symbols: Vec<Symbol>,
    pub timeframe: Timeframe,
    pub from: Timestamp,
    pub to: Timestamp,
    pub feed: Feed,
}

#[derive(Clone)]
pub struct AlpacaClient {
    http: HttpClient,
    config: AlpacaConfig,
}

impl AlpacaClient {
    pub fn new(config: AlpacaConfig) -> Result<Self, SourceError> {
        Ok(Self {
            http: HttpClient::new("buizz", RATE_LIMIT)?,
            config,
        })
    }

    pub async fn bars(
        &self,
        request: &BarsRequest,
        calendar: &Calendar,
    ) -> Result<Vec<Bar>, SourceError> {
        let to = match request.feed {
            Feed::Sip => {
                let delay = TimeDelta::from_std(SIP_DELAY).expect("SIP delay fits a time delta");
                request.to.min(Timestamp::now() - delay)
            }
            Feed::Iex => request.to,
        };
        if to <= request.from || request.symbols.is_empty() {
            return Ok(Vec::new());
        }
        let (timeframe, start, end) = match request.timeframe {
            Timeframe::Day => {
                let sessions = calendar.sessions();
                let closed = sessions.partition_point(|session| session.close <= to);
                let from = request.from.exchange_date();
                let Some(last) = closed
                    .checked_sub(1)
                    .map(|index| sessions[index].date)
                    .filter(|last| *last >= from)
                else {
                    return Ok(Vec::new());
                };
                ("1Day", from.to_string(), last.to_string())
            }
            Timeframe::Minute => ("1Min", request.from.to_string(), to.to_string()),
        };
        let feed = match request.feed {
            Feed::Sip => "sip",
            Feed::Iex => "iex",
        };
        let query = [
            ("symbols", join_symbols(&request.symbols)),
            ("timeframe", timeframe.to_owned()),
            ("start", start),
            ("end", end),
            ("adjustment", "raw".to_owned()),
            ("feed", feed.to_owned()),
            ("limit", MAX_PAGE_LIMIT.to_string()),
            ("sort", "asc".to_owned()),
        ];
        let url = format!("{}/v2/stocks/bars", self.config.data_url);
        let pages = self.pages::<BarsPage>(&url, &query).await?;
        Ok(pages
            .into_iter()
            .flat_map(|page| page.into_bars(request.timeframe, request.feed, calendar))
            .collect())
    }

    pub async fn calendar(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<Session>, SourceError> {
        let query = [("start", from.to_string()), ("end", to.to_string())];
        let url = format!("{}/v2/calendar", self.config.trading_url);
        let days: Vec<WireCalendarDay> = self.get(&url, &query).await?;
        days.into_iter()
            .map(WireCalendarDay::into_session)
            .collect()
    }

    pub async fn assets(&self) -> Result<Vec<Instrument>, SourceError> {
        let query = [("asset_class", "us_equity".to_owned())];
        let url = format!("{}/v2/assets", self.config.trading_url);
        let assets: Vec<WireAsset> = self.get(&url, &query).await?;
        Ok(assets
            .into_iter()
            .filter_map(WireAsset::into_instrument)
            .collect())
    }

    pub async fn corporate_actions(
        &self,
        symbols: &[Symbol],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>, SourceError> {
        let url = format!("{}/v1/corporate-actions", self.config.data_url);
        let mut actions = Vec::new();
        for chunk in symbols.chunks(ACTIONS_SYMBOLS_PER_REQUEST) {
            let query = [
                ("symbols", join_symbols(chunk)),
                (
                    "types",
                    "forward_split,reverse_split,cash_dividend".to_owned(),
                ),
                ("start", from.to_string()),
                ("end", to.to_string()),
                ("limit", ACTIONS_PAGE_LIMIT.to_string()),
                ("sort", "asc".to_owned()),
            ];
            for page in self.pages::<ActionsPage>(&url, &query).await? {
                actions.extend(page.into_actions());
            }
        }
        Ok(sum_dividends_per_ex_date(actions))
    }

    async fn get<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<T, SourceError> {
        let request = self
            .http
            .client()
            .get(url)
            .header("APCA-API-KEY-ID", &self.config.key_id)
            .header("APCA-API-SECRET-KEY", &self.config.secret_key)
            .query(query);
        let body = self.http.send(request).await?.bytes().await?;
        serde_json::from_slice(&body).map_err(|error| SourceError::Decode(error.to_string()))
    }

    async fn pages<T: DeserializeOwned + Paged>(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<T>, SourceError> {
        let mut pages = Vec::new();
        let mut query = query.to_vec();
        loop {
            let mut page: T = self.get(url, &query).await?;
            let token = page
                .take_next_page_token()
                .filter(|token| !token.is_empty());
            pages.push(page);
            let Some(token) = token else {
                return Ok(pages);
            };
            query.retain(|(name, _)| *name != "page_token");
            query.push(("page_token", token));
        }
    }
}

fn sum_dividends_per_ex_date(actions: Vec<CorporateAction>) -> Vec<CorporateAction> {
    let mut dividends: BTreeMap<(Symbol, NaiveDate), Decimal> = BTreeMap::new();
    let mut merged = Vec::with_capacity(actions.len());
    for action in actions {
        match action.kind {
            CorporateActionKind::CashDividend { rate } => {
                *dividends
                    .entry((action.symbol, action.ex_date))
                    .or_default() += rate;
            }
            CorporateActionKind::Split { .. } => merged.push(action),
        }
    }
    merged.extend(
        dividends
            .into_iter()
            .map(|((symbol, ex_date), rate)| CorporateAction {
                symbol,
                ex_date,
                kind: CorporateActionKind::CashDividend { rate },
            }),
    );
    merged
}

fn join_symbols(symbols: &[Symbol]) -> String {
    symbols
        .iter()
        .map(Symbol::as_str)
        .collect::<Vec<_>>()
        .join(",")
}
