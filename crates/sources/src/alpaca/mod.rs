use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

pub use buizz_core::market::SIP_DELAY;
use buizz_core::market::{
    Bar, Calendar, CorporateAction, CorporateActionKind, Feed, Instrument, NewsArticle, Session,
    Symbol, Timeframe,
};
use buizz_core::time::Timestamp;
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;

use crate::http::{HttpClient, REDACTED, RateLimit, SourceError};

mod wire;

pub(crate) use wire::WireNews;
use wire::{ActionsPage, BarsPage, NewsPage, Paged, WireAsset, WireCalendarDay};

pub const DATA_URL: &str = "https://data.alpaca.markets";
pub const TRADING_URL: &str = "https://paper-api.alpaca.markets";
pub const RATE_LIMIT: RateLimit = RateLimit {
    requests: 140,
    per: Duration::from_secs(60),
    cooldown: Duration::from_secs(60),
};
pub use buizz_core::market::ALPACA_HISTORY_START as HISTORY_START;
pub const MAX_PAGE_LIMIT: u32 = 10_000;
pub const ACTIONS_PAGE_LIMIT: u32 = 1_000;
pub const ACTIONS_SYMBOLS_PER_REQUEST: usize = 100;
const ACTION_TYPES: &str = "forward_split,reverse_split,cash_dividend,stock_dividend,spin_off,\
    cash_merger,stock_merger,stock_and_cash_merger,worthless_removal,name_change";
pub const NEWS_PAGE_LIMIT: u32 = 50;
pub const NEWS_HISTORY_START: NaiveDate = NaiveDate::from_ymd_opt(2015, 1, 1).expect("valid date");

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
    pub tickers: Tickers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tickers {
    Today,
    On(NaiveDate),
    AsTraded,
}

pub struct BarPage {
    pub bars: Vec<Bar>,
    pub quiet: Vec<(Symbol, NaiveDate)>,
    pub next: Option<String>,
}

pub struct NewsArticles {
    pub articles: Vec<NewsArticle>,
    pub next: Option<String>,
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
        let mut bars = Vec::new();
        let mut token = None;
        loop {
            let page = self.bars_page(request, calendar, token).await?;
            bars.extend(page.bars);
            match page.next {
                Some(next) => token = Some(next),
                None => return Ok(bars),
            }
        }
    }

    pub async fn bars_page(
        &self,
        request: &BarsRequest,
        calendar: &Calendar,
        token: Option<String>,
    ) -> Result<BarPage, SourceError> {
        let empty = BarPage {
            bars: Vec::new(),
            quiet: Vec::new(),
            next: None,
        };
        let to = match request.feed {
            Feed::Sip => {
                let delay = TimeDelta::from_std(SIP_DELAY).expect("SIP delay fits a time delta");
                request.to.min(Timestamp::now() - delay)
            }
            Feed::Iex => request.to,
        };
        if to <= request.from || request.symbols.is_empty() {
            return Ok(empty);
        }
        let (timeframe, start, end) = match request.timeframe {
            Timeframe::Day => {
                let sessions = calendar.sessions();
                let closed = sessions.partition_point(|session| session.close <= to);
                let from = request.from.exchange_date();
                let Some(last) = closed
                    .checked_sub(1)
                    .map(|index| sessions[index])
                    .filter(|last| last.date >= from)
                else {
                    return Ok(empty);
                };
                ("1Day", from.to_string(), last.close.to_string())
            }
            Timeframe::Minute => ("1Min", request.from.to_string(), to.to_string()),
        };
        let feed = match request.feed {
            Feed::Sip => "sip",
            Feed::Iex => "iex",
        };
        let mut query = vec![
            ("symbols", join_symbols(&request.symbols)),
            ("timeframe", timeframe.to_owned()),
            ("start", start),
            ("end", end),
            ("adjustment", "raw".to_owned()),
            ("feed", feed.to_owned()),
            ("limit", MAX_PAGE_LIMIT.to_string()),
            ("sort", "asc".to_owned()),
        ];
        match request.tickers {
            Tickers::Today => {}
            Tickers::On(date) => query.push(("asof", date.to_string())),
            Tickers::AsTraded => query.push(("asof", "-".to_owned())),
        }
        if let Some(token) = token {
            query.push(("page_token", token));
        }
        let url = format!("{}/v2/stocks/bars", self.config.data_url);
        let mut page: BarsPage = self.get(&url, &query).await?;
        let next = page
            .take_next_page_token()
            .filter(|token| !token.is_empty());
        let (bars, quiet) = page.into_bars(request.timeframe, request.feed, calendar);
        Ok(BarPage { bars, quiet, next })
    }

    pub async fn news_page(
        &self,
        from: Timestamp,
        to: Timestamp,
        token: Option<String>,
    ) -> Result<NewsArticles, SourceError> {
        let mut query = vec![
            ("start", from.to_string()),
            ("end", to.to_string()),
            ("sort", "asc".to_owned()),
            ("limit", NEWS_PAGE_LIMIT.to_string()),
        ];
        if let Some(token) = token {
            query.push(("page_token", token));
        }
        let url = format!("{}/v1beta1/news", self.config.data_url);
        let page: NewsPage = self.get(&url, &query).await?;
        let received_at = Timestamp::now();
        Ok(NewsArticles {
            articles: page
                .news
                .into_iter()
                .map(|news| news.into_article(received_at, true))
                .collect(),
            next: page.next_page_token.filter(|token| !token.is_empty()),
        })
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
        let mut actions = Vec::new();
        for chunk in symbols.chunks(ACTIONS_SYMBOLS_PER_REQUEST) {
            actions.extend(
                self.actions(Some(join_symbols(chunk)), ACTION_TYPES, from, to)
                    .await?,
            );
        }
        Ok(sum_dividends_per_ex_date(actions))
    }

    pub async fn market_corporate_actions(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>, SourceError> {
        Ok(sum_dividends_per_ex_date(
            self.actions(None, ACTION_TYPES, from, to).await?,
        ))
    }

    pub async fn name_changes(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>, SourceError> {
        self.actions(None, "name_change", from, to).await
    }

    pub async fn stock_mergers(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>, SourceError> {
        self.actions(None, "stock_merger", from, to).await
    }

    async fn actions(
        &self,
        symbols: Option<String>,
        types: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<CorporateAction>, SourceError> {
        let url = format!("{}/v1/corporate-actions", self.config.data_url);
        let mut query = vec![
            ("types", types.to_owned()),
            ("start", from.to_string()),
            ("end", to.to_string()),
            ("limit", ACTIONS_PAGE_LIMIT.to_string()),
            ("sort", "asc".to_owned()),
        ];
        if let Some(symbols) = symbols {
            query.push(("symbols", symbols));
        }
        Ok(self
            .pages::<ActionsPage>(&url, &query)
            .await?
            .into_iter()
            .flat_map(ActionsPage::into_actions)
            .collect())
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
        let body = self.http.fetch(request).await?;
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
            _ => merged.push(action),
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
