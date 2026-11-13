use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use serde_json::{Map, Value};

use crate::api::ContractState;
use crate::ledger::Portfolio;
use crate::market::{
    Adjustment, Borrow, CorporateAction, DataSource, Dataset, EarningsRelease, EtfHolding,
    EtfShares, FactStatus, FailToDeliver, Filing, FomcAnnouncement, FundamentalMetric,
    FundamentalPoint, InsiderTrade, InstrumentProfile, MacroPoint, MacroSeries, NewsArticle,
    OptionQuote, PageViews, PressRelease, Session, ShortInterest, ShortVolume, Symbol, Timeframe,
    TradingHalt,
};
use crate::order::{Fill, Order, OrderId, OrderRequest, OrderType, OrderUpdate, Side};
use crate::run::{LogLevel, Mode, RunId, UniverseSpec, Wake};
use crate::time::Timestamp;

pub const PROTOCOL_VERSION: u32 = 1;
pub const BOT_PATH: &str = "/bot/v1";
pub const MAX_STATE_BYTES: usize = 64 * 1024;
pub const ENV_SPINE_URL: &str = "BUIZZ_SPINE_URL";
pub const ENV_RUN_ID: &str = "BUIZZ_RUN_ID";
pub const ENV_RUN_TOKEN: &str = "BUIZZ_RUN_TOKEN";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotMessage {
    Hello(Hello),
    Query(Query),
    Ack(Ack),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpineMessage {
    Ready(Box<Ready>),
    Step(Box<Step>),
    Data(Data),
    Error(ProtocolError),
    End(End),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub sdk: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ready {
    pub run_id: RunId,
    pub bot: String,
    pub mode: Mode,
    pub timeframe: Timeframe,
    pub wake: Wake,
    pub params: Map<String, Value>,
    pub universe: Vec<Symbol>,
    pub start: NaiveDate,
    pub end: Option<NaiveDate>,
    pub seed: u64,
    pub state: Option<ByteBuf>,
    pub cash: Decimal,
    pub equity: Decimal,
    pub holdings: Holdings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub seq: u64,
    pub clock: Timestamp,
    pub session: Session,
    pub universe: Option<Vec<Symbol>>,
    pub bars: BarBuffer,
    pub events: Vec<StepEvent>,
    pub cash: Decimal,
    pub equity: Decimal,
    pub holdings: Option<Holdings>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepEvent {
    Fill(Fill),
    Order(OrderUpdate),
    CorporateAction(CorporateAction),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub id: u32,
    pub request: QueryRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryRequest {
    History {
        symbols: Vec<Symbol>,
        timeframe: Timeframe,
        bars: u32,
        adjust: Adjustment,
    },
    Fundamentals {
        symbols: Vec<Symbol>,
        metrics: Vec<FundamentalMetric>,
    },
    Instruments {
        symbols: Vec<Symbol>,
    },
    Earnings {
        symbols: Vec<Symbol>,
    },
    Filings {
        symbols: Vec<Symbol>,
        since: NaiveDate,
        items: Option<Vec<String>>,
    },
    PressReleases {
        symbols: Vec<Symbol>,
        since: NaiveDate,
        accessions: Option<Vec<String>>,
    },
    Calendar {
        from: NaiveDate,
        to: NaiveDate,
    },
    Universe {
        date: NaiveDate,
        universe: UniverseSpec,
    },
    Macro {
        series: Vec<MacroSeries>,
        from: NaiveDate,
        to: NaiveDate,
    },
    InsiderTrades {
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
    News {
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
    Fomc {
        from: NaiveDate,
        to: NaiveDate,
    },
    ShortInterest {
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
    ShortVolume {
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
    Borrow {
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
    OptionChains {
        underlyings: Vec<Symbol>,
        since: NaiveDate,
    },
    TradingHalts {
        symbols: Option<Vec<Symbol>>,
        since: NaiveDate,
    },
    EtfHoldings {
        etfs: Vec<Symbol>,
        since: NaiveDate,
    },
    FailsToDeliver {
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
    PageViews {
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
    Completeness {
        datasets: Vec<Dataset>,
        from: NaiveDate,
        to: NaiveDate,
    },
    Facts {
        recipe: String,
        symbols: Vec<Symbol>,
        since: NaiveDate,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Data {
    pub id: u32,
    pub result: DataResult,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataResult {
    History {
        bars: BarBuffer,
    },
    Fundamentals {
        points: Vec<FundamentalPoint>,
    },
    Instruments {
        instruments: Vec<InstrumentProfile>,
    },
    Earnings {
        releases: Vec<EarningsRelease>,
    },
    Filings {
        filings: Vec<Filing>,
    },
    PressReleases {
        releases: Vec<PressRelease>,
    },
    Calendar {
        sessions: Vec<Session>,
    },
    Universe {
        symbols: Vec<Symbol>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reliable_from: Option<NaiveDate>,
    },
    Macro {
        points: Vec<MacroPoint>,
    },
    InsiderTrades {
        trades: Vec<InsiderTrade>,
    },
    News {
        articles: Vec<NewsArticle>,
    },
    Fomc {
        announcements: Vec<FomcAnnouncement>,
    },
    ShortInterest {
        positions: Vec<ShortInterest>,
    },
    ShortVolume {
        volumes: Vec<ShortVolume>,
    },
    Borrow {
        snapshots: Vec<Borrow>,
    },
    OptionChains {
        quotes: Vec<OptionQuote>,
    },
    TradingHalts {
        halts: Vec<TradingHalt>,
    },
    EtfHoldings {
        holdings: Vec<EtfHolding>,
        shares: Vec<EtfShares>,
    },
    FailsToDeliver {
        fails: Vec<FailToDeliver>,
    },
    PageViews {
        views: Vec<PageViews>,
    },
    Completeness {
        series: Vec<SeriesCompleteness>,
    },
    Facts {
        facts: Vec<FactRow>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactRow {
    pub symbol: Symbol,
    pub accession: String,
    pub status: FactStatus,
    pub available_at: Timestamp,
    pub evidence: Option<String>,
    pub fields: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesCompleteness {
    pub dataset: Dataset,
    pub source: DataSource,
    pub complete: bool,
    pub complete_through: Option<NaiveDate>,
    pub state: ContractState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub id: Option<u32>,
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    VersionMismatch,
    ProtocolViolation,
    BadQuery,
    StateTooLarge,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    pub seq: u64,
    #[serde(default)]
    pub orders: Vec<OrderRequest>,
    #[serde(default)]
    pub cancels: Vec<String>,
    #[serde(default)]
    pub state: Option<ByteBuf>,
    #[serde(default)]
    pub logs: Vec<LogLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub level: LogLevel,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    pub reason: EndReason,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Completed,
    Paused,
    Stopped,
    RiskLimit,
    Failed,
    SessionEnd,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holdings {
    pub positions: Vec<PositionSnapshot>,
    pub working: Vec<WorkingOrder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub symbol: Symbol,
    pub qty: i64,
    pub avg_cost: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingOrder {
    pub order_id: OrderId,
    pub client_id: String,
    pub symbol: Symbol,
    pub side: Side,
    pub qty: u64,
    pub filled_qty: u64,
    pub order_type: OrderType,
}

impl Holdings {
    pub fn new<'a>(portfolio: &Portfolio, working: impl IntoIterator<Item = &'a Order>) -> Self {
        Self {
            positions: portfolio
                .open_positions()
                .map(|(symbol, position)| PositionSnapshot {
                    symbol: symbol.clone(),
                    qty: position.qty,
                    avg_cost: position.average_cost(),
                })
                .collect(),
            working: working
                .into_iter()
                .map(|order| WorkingOrder {
                    order_id: order.id,
                    client_id: order.client_id.clone(),
                    symbol: order.symbol.clone(),
                    side: order.side,
                    qty: order.qty,
                    filled_qty: order.filled_qty,
                    order_type: order.order_type,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarRow {
    pub symbol: Symbol,
    pub start: Timestamp,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarBuffer {
    pub symbols: Vec<Symbol>,
    pub symbol_index: ByteBuf,
    pub start: ByteBuf,
    pub open: ByteBuf,
    pub high: ByteBuf,
    pub low: ByteBuf,
    pub close: ByteBuf,
    pub volume: ByteBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum BarBufferError {
    #[error("bar buffer column {0} has the wrong length")]
    ColumnLength(&'static str),
    #[error("bar buffer symbol index {0} is out of range")]
    SymbolIndex(u16),
    #[error("bar buffer has more than {} symbols", u16::MAX)]
    TooManySymbols,
}

impl BarBuffer {
    pub fn from_rows<'a>(
        rows: impl IntoIterator<Item = &'a BarRow>,
    ) -> Result<Self, BarBufferError> {
        let mut buffer = Self::default();
        let mut indexes: HashMap<&Symbol, u16> = HashMap::new();
        for row in rows {
            let index = match indexes.get(&row.symbol) {
                Some(index) => *index,
                None => {
                    let index = u16::try_from(buffer.symbols.len())
                        .map_err(|_| BarBufferError::TooManySymbols)?;
                    buffer.symbols.push(row.symbol.clone());
                    indexes.insert(&row.symbol, index);
                    index
                }
            };
            buffer.symbol_index.extend_from_slice(&index.to_le_bytes());
            buffer
                .start
                .extend_from_slice(&row.start.nanos().to_le_bytes());
            buffer.open.extend_from_slice(&row.open.to_le_bytes());
            buffer.high.extend_from_slice(&row.high.to_le_bytes());
            buffer.low.extend_from_slice(&row.low.to_le_bytes());
            buffer.close.extend_from_slice(&row.close.to_le_bytes());
            buffer.volume.extend_from_slice(&row.volume.to_le_bytes());
        }
        Ok(buffer)
    }

    pub fn len(&self) -> usize {
        self.symbol_index.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.symbol_index.is_empty()
    }

    pub fn rows(&self) -> Result<Vec<BarRow>, BarBufferError> {
        let len = self.len();
        let columns: [(&'static str, &ByteBuf, usize); 7] = [
            ("symbol_index", &self.symbol_index, 2),
            ("start", &self.start, 8),
            ("open", &self.open, 8),
            ("high", &self.high, 8),
            ("low", &self.low, 8),
            ("close", &self.close, 8),
            ("volume", &self.volume, 8),
        ];
        for (name, column, width) in columns {
            if column.len() != len * width {
                return Err(BarBufferError::ColumnLength(name));
            }
        }
        let float = |column: &ByteBuf, row: usize| {
            f64::from_le_bytes(
                column[row * 8..row * 8 + 8]
                    .try_into()
                    .expect("8-byte slice"),
            )
        };
        (0..len)
            .map(|row| {
                let index = u16::from_le_bytes(
                    self.symbol_index[row * 2..row * 2 + 2]
                        .try_into()
                        .expect("2-byte slice"),
                );
                let symbol = self
                    .symbols
                    .get(usize::from(index))
                    .ok_or(BarBufferError::SymbolIndex(index))?;
                Ok(BarRow {
                    symbol: symbol.clone(),
                    start: Timestamp::from_nanos(i64::from_le_bytes(
                        self.start[row * 8..row * 8 + 8]
                            .try_into()
                            .expect("8-byte slice"),
                    )),
                    open: float(&self.open, row),
                    high: float(&self.high, row),
                    low: float(&self.low, row),
                    close: float(&self.close, row),
                    volume: float(&self.volume, row),
                })
            })
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("cannot encode protocol frame: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("cannot decode protocol frame: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
}

pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, CodecError> {
    Ok(rmp_serde::to_vec_named(message)?)
}

pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T, CodecError> {
    Ok(rmp_serde::from_slice(frame)?)
}
