use std::path::PathBuf;
use std::str::FromStr;

use buizz_core::api::ContractState;
use buizz_core::market::{
    Adjustment, Borrow, CorporateAction, CorporateActionKind, DataSource, Dataset, Feed, Filing,
    FomcAnnouncement, FundamentalMetric, FundamentalPoint, InsiderOwner, InsiderTrade,
    InsiderTransaction, MacroPoint, MacroSeries, NewsArticle, PeriodType, PressRelease, Session,
    ShortInterest, ShortVolume, Symbol, Timeframe,
};
use buizz_core::order::{
    Fill, OrderId, OrderRequest, OrderSize, OrderStatus, OrderType, OrderUpdate, Side, TimeInForce,
    Trail,
};
use buizz_core::protocol::{
    Ack, BarBuffer, BarRow, BotMessage, Data, DataResult, End, EndReason, ErrorCode, Hello,
    Holdings, LogLine, PROTOCOL_VERSION, PositionSnapshot, ProtocolError, Query, QueryRequest,
    Ready, SeriesCompleteness, SpineMessage, Step, StepEvent, WorkingOrder, encode,
};
use buizz_core::run::{LogLevel, Mode, RunId, UniverseSpec, Wake};
use buizz_core::time::Timestamp;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_bytes::ByteBuf;
use serde_json::{Map, Value, json};

fn at(text: &str) -> Timestamp {
    text.parse().expect("valid timestamp")
}

fn date(text: &str) -> NaiveDate {
    text.parse().expect("valid date")
}

fn money(text: &str) -> Decimal {
    Decimal::from_str(text).expect("valid decimal")
}

fn session(day: &str, close: &str) -> Session {
    Session {
        date: date(day),
        open: at(&format!("{day}T14:30:00Z")),
        close: at(&format!("{day}T{close}Z")),
    }
}

fn row(symbol: &str, start: &str, ohlcv: [f64; 5]) -> BarRow {
    BarRow {
        symbol: Symbol::new(symbol),
        start: at(start),
        open: ohlcv[0],
        high: ohlcv[1],
        low: ohlcv[2],
        close: ohlcv[3],
        volume: ohlcv[4],
    }
}

fn bars(rows: &[BarRow]) -> BarBuffer {
    BarBuffer::from_rows(rows).expect("small buffer")
}

fn params() -> Map<String, Value> {
    match json!({ "weight": 0.25 }) {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

fn render(value: &rmpv::Value) -> Value {
    match value {
        rmpv::Value::Nil => Value::Null,
        rmpv::Value::Boolean(flag) => Value::Bool(*flag),
        rmpv::Value::Integer(int) => int
            .as_i64()
            .map(Value::from)
            .or_else(|| int.as_u64().map(Value::from))
            .unwrap_or(Value::Null),
        rmpv::Value::F32(float) => json!(float),
        rmpv::Value::F64(float) => json!(float),
        rmpv::Value::String(text) => Value::String(text.as_str().unwrap_or_default().to_owned()),
        rmpv::Value::Binary(bytes) => {
            json!({ "bin": bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>() })
        }
        rmpv::Value::Array(items) => Value::Array(items.iter().map(render).collect()),
        rmpv::Value::Map(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.as_str().unwrap_or_default().to_owned(), render(value)))
                .collect(),
        ),
        rmpv::Value::Ext(_, _) => Value::Null,
    }
}

fn write<T: Serialize>(dir: &std::path::Path, name: &str, message: &T) {
    let frame = encode(message).expect("frame encodes");
    let value = rmpv::decode::read_value(&mut frame.as_slice()).expect("frame is msgpack");
    std::fs::write(dir.join(format!("{name}.msgpack")), &frame).expect("write frame");
    let pretty = serde_json::to_string_pretty(&render(&value)).expect("render json");
    std::fs::write(dir.join(format!("{name}.json")), pretty + "\n").expect("write json");
}

fn main() {
    let dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: golden_frames <output dir>");
    std::fs::create_dir_all(&dir).expect("create output dir");

    let aaa = Symbol::new("AAA");
    let bbb = Symbol::new("BBB");
    let universe = vec![aaa.clone(), bbb.clone()];

    write(
        &dir,
        "hello",
        &BotMessage::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            sdk: "buizz-python/0.1.0".to_owned(),
        }),
    );

    write(
        &dir,
        "ready_backtest",
        &SpineMessage::Ready(Box::new(Ready {
            run_id: RunId(41),
            bot: "red_green".to_owned(),
            mode: Mode::Backtest,
            timeframe: Timeframe::Day,
            wake: Wake::Bar,
            params: params(),
            universe: universe.clone(),
            start: date("2024-11-20"),
            end: Some(date("2024-12-04")),
            seed: 7,
            state: None,
            cash: money("100000.00"),
            equity: money("100000.00"),
            holdings: Holdings::default(),
        })),
    );

    write(
        &dir,
        "ready_live",
        &SpineMessage::Ready(Box::new(Ready {
            run_id: RunId(42),
            bot: "opening_range".to_owned(),
            mode: Mode::Live,
            timeframe: Timeframe::Minute,
            wake: "every:30m".parse().expect("valid wake"),
            params: params(),
            universe: universe.clone(),
            start: date("2024-11-29"),
            end: None,
            seed: 7,
            state: Some(ByteBuf::from(br#"{"range_high":99.53}"#.to_vec())),
            cash: money("75104.63"),
            equity: money("99854.63"),
            holdings: Holdings {
                positions: vec![PositionSnapshot {
                    symbol: aaa.clone(),
                    qty: 250,
                    avg_cost: money("99.0495"),
                }],
                working: vec![WorkingOrder {
                    order_id: OrderId(5),
                    client_id: "AAA-2024-11-27".to_owned(),
                    symbol: aaa.clone(),
                    side: Side::Sell,
                    qty: 250,
                    filled_qty: 0,
                    order_type: OrderType::Market,
                }],
            },
        })),
    );

    write(
        &dir,
        "step_backtest",
        &SpineMessage::Step(Box::new(Step {
            seq: 1,
            clock: at("2024-11-21T21:00:00Z"),
            session: session("2024-11-21", "21:00:00"),
            universe: None,
            bars: bars(&[
                row(
                    "AAA",
                    "2024-11-21T14:30:00Z",
                    [203.0, 204.0, 198.4, 199.0, 2_400_000.0],
                ),
                row(
                    "BBB",
                    "2024-11-21T14:30:00Z",
                    [49.2, 49.6, 48.6, 48.8, 1_100_000.0],
                ),
            ]),
            events: vec![
                StepEvent::Fill(Fill {
                    order_id: OrderId(2),
                    client_id: "BBB-2024-11-20".to_owned(),
                    symbol: bbb.clone(),
                    at: at("2024-11-21T14:30:00Z"),
                    qty: 510,
                    price: money("49.2246"),
                    commission: money("2.55"),
                    feed: Feed::Sip,
                }),
                StepEvent::Order(OrderUpdate {
                    order_id: OrderId(2),
                    client_id: "BBB-2024-11-20".to_owned(),
                    symbol: bbb.clone(),
                    status: OrderStatus::Filled,
                    reject_reason: None,
                    filled_qty: 510,
                    at: at("2024-11-21T14:30:00Z"),
                }),
            ],
            cash: money("74892.90"),
            equity: money("99780.90"),
            holdings: Some(Holdings {
                positions: vec![PositionSnapshot {
                    symbol: bbb.clone(),
                    qty: 510,
                    avg_cost: money("49.2246"),
                }],
                working: Vec::new(),
            }),
        })),
    );

    write(
        &dir,
        "step_live",
        &SpineMessage::Step(Box::new(Step {
            seq: 318,
            clock: at("2024-12-02T15:02:00Z"),
            session: session("2024-12-02", "21:00:00"),
            universe: Some(universe.clone()),
            bars: bars(&[
                row(
                    "AAA",
                    "2024-12-02T15:00:00Z",
                    [100.62, 100.64, 100.61, 100.64, 9_487.0],
                ),
                row(
                    "BBB",
                    "2024-12-02T15:00:00Z",
                    [49.06, 49.07, 49.06, 49.07, 2_615.0],
                ),
                row(
                    "AAA",
                    "2024-12-02T15:01:00Z",
                    [100.64, 100.66, 100.64, 100.66, 9_487.0],
                ),
                row(
                    "BBB",
                    "2024-12-02T15:01:00Z",
                    [49.07, 49.08, 49.07, 49.08, 2_615.0],
                ),
            ]),
            events: vec![StepEvent::CorporateAction(CorporateAction {
                symbol: aaa.clone(),
                ex_date: date("2024-12-02"),
                kind: CorporateActionKind::CashDividend {
                    rate: money("0.24"),
                },
            })],
            cash: money("99965.93"),
            equity: money("99965.93"),
            holdings: None,
        })),
    );

    write(
        &dir,
        "query_history",
        &BotMessage::Query(Query {
            id: 1,
            request: QueryRequest::History {
                symbols: vec![aaa.clone()],
                timeframe: Timeframe::Day,
                bars: 2,
                adjust: Adjustment::Split,
            },
        }),
    );

    write(
        &dir,
        "query_fundamentals",
        &BotMessage::Query(Query {
            id: 2,
            request: QueryRequest::Fundamentals {
                symbols: vec![bbb.clone()],
                metrics: vec![
                    FundamentalMetric::NetIncome,
                    FundamentalMetric::SharesOutstanding,
                ],
            },
        }),
    );

    write(
        &dir,
        "data_history",
        &SpineMessage::Data(Data {
            id: 1,
            result: DataResult::History {
                bars: bars(&[
                    row(
                        "AAA",
                        "2024-11-25T14:30:00Z",
                        [98.75, 99.1, 97.55, 98.0, 3_900_000.0],
                    ),
                    row(
                        "AAA",
                        "2024-11-26T14:30:00Z",
                        [98.5, 99.2, 96.4, 97.0, 4_300_000.0],
                    ),
                ]),
            },
        }),
    );

    write(
        &dir,
        "data_fundamentals",
        &SpineMessage::Data(Data {
            id: 2,
            result: DataResult::Fundamentals {
                points: vec![FundamentalPoint {
                    symbol: bbb.clone(),
                    metric: FundamentalMetric::NetIncome,
                    period_type: PeriodType::Annual,
                    period_start: Some(date("2023-01-01")),
                    period_end: date("2023-12-31"),
                    value: money("410000000"),
                    unit: "USD".to_owned(),
                    form: "10-K".to_owned(),
                    filed: date("2024-02-15"),
                    available_at: at("2024-02-16T14:30:00Z"),
                }],
            },
        }),
    );

    write(
        &dir,
        "error",
        &SpineMessage::Error(ProtocolError {
            id: Some(3),
            code: ErrorCode::BadQuery,
            message: "history bars must be between 1 and 10000".to_owned(),
        }),
    );

    write(
        &dir,
        "query_calendar",
        &BotMessage::Query(Query {
            id: 4,
            request: QueryRequest::Calendar {
                from: date("2024-11-28"),
                to: date("2024-12-02"),
            },
        }),
    );

    write(
        &dir,
        "data_calendar",
        &SpineMessage::Data(Data {
            id: 4,
            result: DataResult::Calendar {
                sessions: vec![
                    session("2024-11-29", "18:00:00"),
                    session("2024-12-02", "21:00:00"),
                ],
            },
        }),
    );

    write(
        &dir,
        "query_universe",
        &BotMessage::Query(Query {
            id: 5,
            request: QueryRequest::Universe {
                date: date("2024-11-22"),
                universe: UniverseSpec::TopDollarVolume {
                    top_dollar_volume: 2,
                    asset_class: None,
                },
            },
        }),
    );

    write(
        &dir,
        "data_universe",
        &SpineMessage::Data(Data {
            id: 5,
            result: DataResult::Universe {
                symbols: vec![Symbol::new("SPY"), aaa.clone()],
                reliable_from: None,
            },
        }),
    );

    write(
        &dir,
        "query_macro",
        &BotMessage::Query(Query {
            id: 6,
            request: QueryRequest::Macro {
                series: vec![MacroSeries::Treasury3m, MacroSeries::Cpiaucns],
                from: date("2024-10-01"),
                to: date("2024-11-21"),
            },
        }),
    );

    write(
        &dir,
        "data_macro",
        &SpineMessage::Data(Data {
            id: 6,
            result: DataResult::Macro {
                points: vec![
                    MacroPoint {
                        series: MacroSeries::Treasury3m,
                        date: date("2024-11-20"),
                        value: money("4.50"),
                        published: date("2024-11-20"),
                        available_at: at("2024-11-21T14:30:00Z"),
                    },
                    MacroPoint {
                        series: MacroSeries::Cpiaucns,
                        date: date("2024-10-01"),
                        value: money("315.664"),
                        published: date("2024-11-13"),
                        available_at: at("2024-11-14T14:30:00Z"),
                    },
                ],
            },
        }),
    );

    write(
        &dir,
        "query_insider_trades",
        &BotMessage::Query(Query {
            id: 7,
            request: QueryRequest::InsiderTrades {
                symbols: vec![bbb.clone()],
                since: date("2024-10-01"),
            },
        }),
    );

    write(
        &dir,
        "data_insider_trades",
        &SpineMessage::Data(Data {
            id: 7,
            result: DataResult::InsiderTrades {
                trades: vec![InsiderTrade {
                    symbol: bbb.clone(),
                    accession: "0001140361-24-041201".to_owned(),
                    form: "4".to_owned(),
                    filed: date("2024-11-19"),
                    accepted_at: at("2024-11-19T22:30:14Z"),
                    available_at: at("2024-11-20T14:30:00Z"),
                    plan_10b5_1: Some(false),
                    owners: vec![InsiderOwner {
                        cik: 1214156,
                        name: "Doe Jane".to_owned(),
                        director: true,
                        officer: true,
                        ten_percent_owner: false,
                        other: false,
                        officer_title: Some("Chief Executive Officer".to_owned()),
                    }],
                    transaction: InsiderTransaction {
                        security: "Common Stock".to_owned(),
                        date: date("2024-11-18"),
                        code: "P".to_owned(),
                        acquired: true,
                        shares: Some(money("1000")),
                        price: Some(money("49.1250")),
                        shares_after: Some(money("25000")),
                        direct: true,
                    },
                }],
            },
        }),
    );

    write(
        &dir,
        "query_news",
        &BotMessage::Query(Query {
            id: 8,
            request: QueryRequest::News {
                symbols: vec![bbb.clone()],
                since: date("2024-11-20"),
            },
        }),
    );

    write(
        &dir,
        "data_news",
        &SpineMessage::Data(Data {
            id: 8,
            result: DataResult::News {
                articles: vec![NewsArticle {
                    id: 42_133_817,
                    symbols: vec![bbb.clone()],
                    headline: "BBB CEO Buys Shares".to_owned(),
                    summary: String::new(),
                    source: "benzinga".to_owned(),
                    created_at: at("2024-11-20T15:02:10Z"),
                    updated_at: at("2024-11-20T15:02:10Z"),
                    received_at: at("2024-11-20T15:02:11Z"),
                    backfilled: false,
                }],
            },
        }),
    );

    write(
        &dir,
        "query_fomc",
        &BotMessage::Query(Query {
            id: 9,
            request: QueryRequest::Fomc {
                from: date("2024-11-21"),
                to: date("2024-12-31"),
            },
        }),
    );

    write(
        &dir,
        "data_fomc",
        &SpineMessage::Data(Data {
            id: 9,
            result: DataResult::Fomc {
                announcements: vec![FomcAnnouncement {
                    date: date("2024-12-18"),
                    scheduled: true,
                    statement_at: Some(at("2024-12-18T19:00:00Z")),
                    available_at: at("2024-01-01T05:00:00Z"),
                }],
            },
        }),
    );

    write(
        &dir,
        "query_filings",
        &BotMessage::Query(Query {
            id: 10,
            request: QueryRequest::Filings {
                symbols: vec![bbb.clone()],
                since: date("2024-11-01"),
                items: Some(vec!["2.02".to_owned(), "4.02".to_owned()]),
            },
        }),
    );

    write(
        &dir,
        "data_filings",
        &SpineMessage::Data(Data {
            id: 10,
            result: DataResult::Filings {
                filings: vec![Filing {
                    symbol: bbb.clone(),
                    cik: 9_999_001,
                    accession: "0009999001-24-000050".to_owned(),
                    form: "8-K".to_owned(),
                    filed: date("2024-11-20"),
                    report_date: Some(date("2024-11-20")),
                    items: vec!["2.02".to_owned(), "9.01".to_owned()],
                    accepted_at: at("2024-11-20T21:05:12Z"),
                    available_at: at("2024-11-20T21:05:12Z"),
                }],
            },
        }),
    );

    write(
        &dir,
        "query_press_releases",
        &BotMessage::Query(Query {
            id: 11,
            request: QueryRequest::PressReleases {
                symbols: vec![bbb.clone()],
                since: date("2024-11-01"),
                accessions: None,
            },
        }),
    );

    write(
        &dir,
        "data_press_releases",
        &SpineMessage::Data(Data {
            id: 11,
            result: DataResult::PressReleases {
                releases: vec![PressRelease {
                    symbol: bbb.clone(),
                    accession: "0009999001-24-000050".to_owned(),
                    accepted_at: at("2024-11-20T21:05:12Z"),
                    available_at: at("2024-11-20T21:05:12Z"),
                    exhibit: "EX-99.1".to_owned(),
                    text: "Beta Fixture reports third quarter results\n\nRevenue\t$\t1,250"
                        .to_owned(),
                }],
            },
        }),
    );

    write(
        &dir,
        "query_short_interest",
        &BotMessage::Query(Query {
            id: 12,
            request: QueryRequest::ShortInterest {
                symbols: vec![aaa.clone()],
                since: date("2024-11-01"),
            },
        }),
    );

    write(
        &dir,
        "data_short_interest",
        &SpineMessage::Data(Data {
            id: 12,
            result: DataResult::ShortInterest {
                positions: vec![ShortInterest {
                    symbol: aaa.clone(),
                    settlement: date("2024-11-15"),
                    shares: 4_210_000,
                    previous_shares: 3_980_500,
                    average_daily_volume: 2_350_000,
                    days_to_cover: money("1.79"),
                    split: false,
                    revised: false,
                    published: date("2024-11-26"),
                    available_at: at("2024-11-27T14:30:00Z"),
                }],
            },
        }),
    );

    write(
        &dir,
        "query_short_volume",
        &BotMessage::Query(Query {
            id: 13,
            request: QueryRequest::ShortVolume {
                symbols: vec![aaa.clone()],
                since: date("2024-11-20"),
            },
        }),
    );

    write(
        &dir,
        "data_short_volume",
        &SpineMessage::Data(Data {
            id: 13,
            result: DataResult::ShortVolume {
                volumes: vec![ShortVolume {
                    symbol: aaa.clone(),
                    date: date("2024-11-20"),
                    short_volume: money("812345.5"),
                    short_exempt_volume: money("1200"),
                    total_volume: money("1650000"),
                    available_at: at("2024-11-21T14:30:00Z"),
                }],
            },
        }),
    );

    write(
        &dir,
        "query_borrow",
        &BotMessage::Query(Query {
            id: 14,
            request: QueryRequest::Borrow {
                symbols: vec![aaa.clone()],
                since: date("2024-11-20"),
            },
        }),
    );

    write(
        &dir,
        "data_borrow",
        &SpineMessage::Data(Data {
            id: 14,
            result: DataResult::Borrow {
                snapshots: vec![Borrow {
                    symbol: aaa.clone(),
                    at: at("2024-11-20T20:45:12Z"),
                    rebate_rate: Some(money("3.9188")),
                    fee_rate: Some(money("0.25")),
                    available: 3_500_000,
                    available_or_more: true,
                    available_at: at("2024-11-20T21:02:40Z"),
                }],
            },
        }),
    );

    write(
        &dir,
        "query_completeness",
        &BotMessage::Query(Query {
            id: 15,
            request: QueryRequest::Completeness {
                datasets: vec![Dataset::News],
                from: date("2019-01-01"),
                to: date("2024-11-20"),
            },
        }),
    );

    write(
        &dir,
        "data_completeness",
        &SpineMessage::Data(Data {
            id: 15,
            result: DataResult::Completeness {
                series: vec![SeriesCompleteness {
                    dataset: Dataset::News,
                    source: DataSource::Alpaca,
                    complete: false,
                    complete_through: Some(date("2020-10-31")),
                    state: ContractState::Filling,
                }],
            },
        }),
    );

    write(
        &dir,
        "ack_backtest",
        &BotMessage::Ack(Ack {
            seq: 1,
            orders: vec![
                OrderRequest {
                    client_id: "AAA-2024-11-21".to_owned(),
                    symbol: aaa.clone(),
                    size: OrderSize::TargetWeight {
                        weight: money("0.25"),
                    },
                    order_type: OrderType::Market,
                    tif: TimeInForce::Day,
                    reason: "red day".to_owned(),
                },
                OrderRequest {
                    client_id: "BBB-limit-1".to_owned(),
                    symbol: bbb.clone(),
                    size: OrderSize::Qty { qty: -100 },
                    order_type: OrderType::Limit {
                        limit_price: money("49.60"),
                    },
                    tif: TimeInForce::Day,
                    reason: "take profit".to_owned(),
                },
            ],
            cancels: vec!["BBB-limit-0".to_owned()],
            state: Some(ByteBuf::from(br#"{"days_red":1}"#.to_vec())),
            logs: vec![LogLine {
                level: LogLevel::Info,
                message: "AAA closed red".to_owned(),
            }],
        }),
    );

    write(
        &dir,
        "ack_live",
        &BotMessage::Ack(Ack {
            seq: 318,
            orders: vec![
                OrderRequest {
                    client_id: "AAA-2024-12-02".to_owned(),
                    symbol: aaa.clone(),
                    size: OrderSize::TargetWeight {
                        weight: money("0.5"),
                    },
                    order_type: OrderType::MarketOnClose,
                    tif: TimeInForce::Day,
                    reason: "trade the close".to_owned(),
                },
                OrderRequest {
                    client_id: "AAA-trail".to_owned(),
                    symbol: aaa.clone(),
                    size: OrderSize::Qty { qty: -10 },
                    order_type: OrderType::TrailingStop {
                        trail: Trail::Percent {
                            percent: money("0.05"),
                        },
                        stop_price: None,
                    },
                    tif: TimeInForce::Gtc,
                    reason: "protect the gain".to_owned(),
                },
                OrderRequest {
                    client_id: "BBB-breakout".to_owned(),
                    symbol: bbb.clone(),
                    size: OrderSize::Qty { qty: 20 },
                    order_type: OrderType::Stop {
                        stop_price: money("50.25"),
                    },
                    tif: TimeInForce::Gtc,
                    reason: "breakout".to_owned(),
                },
            ],
            cancels: Vec::new(),
            state: None,
            logs: Vec::new(),
        }),
    );

    write(
        &dir,
        "end_backtest",
        &SpineMessage::End(End {
            reason: EndReason::Completed,
            message: None,
        }),
    );

    write(
        &dir,
        "end_live",
        &SpineMessage::End(End {
            reason: EndReason::Paused,
            message: Some("paused from the UI".to_owned()),
        }),
    );
}
