use buizz_core::ledger::{LedgerEntry, LedgerKind};
use buizz_core::market::{Feed, Symbol, Timeframe};
use buizz_core::ops::JobOutcome;
use buizz_core::order::{Order, OrderId, OrderSize, OrderStatus, OrderType, Side, TimeInForce};
use buizz_core::run::{
    CostModel, DesiredState, EquityPoint, Mode, RiskLimits, RunConfig, RunEvent, RunEventKind,
    RunStatus, UniverseSpec, Wake,
};
use buizz_core::time::Timestamp;
use buizz_state::{FILE_NAME, NewRun, StateError, StateStore, StepCommit};
use chrono::NaiveDate;
use rust_decimal::Decimal;

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn at(day: i64) -> Timestamp {
    Timestamp::from_nanos(day * 86_400_000_000_000)
}

fn new_run(mode: Mode) -> NewRun {
    NewRun {
        config: RunConfig {
            bot: "momentum".into(),
            mode,
            params: serde_json::Map::new(),
            universe: UniverseSpec::Symbols {
                symbols: vec![Symbol::new("AAPL")],
            },
            timeframe: Timeframe::Day,
            wake: Wake::Bar,
            start: NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            end: None,
            start_cash: d("100000"),
            cost: CostModel::default(),
            risk: RiskLimits::default(),
            seed: 7,
            benchmark: Symbol::new("SPY"),
            holdout: false,
        },
        status: RunStatus::Running,
        desired: DesiredState::Running,
        bot_hash: "abc".into(),
        engine_version: "1".into(),
        data_watermark: None,
        badges: vec![],
        created_at: at(1),
    }
}

fn order(id: u64, client_id: &str, status: OrderStatus, filled_qty: u64) -> Order {
    Order {
        id: OrderId(id),
        client_id: client_id.into(),
        symbol: Symbol::new("AAPL"),
        side: Side::Buy,
        qty: 10,
        filled_qty,
        requested: OrderSize::Qty { qty: 10 },
        order_type: OrderType::Market,
        tif: TimeInForce::Day,
        reason: "signal".into(),
        decided_at: at(2),
        session: None,
        status,
        reject_reason: None,
        updated_at: at(3),
    }
}

fn step(run_id: buizz_core::run::RunId, n: u64, orders: Vec<Order>) -> StepCommit {
    let seq = n * 10;
    StepCommit {
        run_id,
        step_seq: n,
        clock: at(n as i64 + 1),
        equity: d("100000.25") + Decimal::from(n),
        ledger: vec![LedgerEntry {
            seq,
            at: at(n as i64 + 1),
            kind: if n == 1 {
                LedgerKind::Deposit {
                    amount: d("100000"),
                }
            } else {
                LedgerKind::Fill {
                    order_id: OrderId(n),
                    symbol: Symbol::new("AAPL"),
                    qty: 10,
                    price: d("150.125"),
                    commission: d("1"),
                    feed: Feed::Sip,
                }
            },
        }],
        orders,
        equity_points: vec![EquityPoint {
            at: at(n as i64 + 1),
            equity: d("100000.25") + Decimal::from(n),
            cash: d("98000.5"),
            gross_exposure: d("1501.25"),
        }],
        events: vec![RunEvent {
            seq,
            at: at(n as i64 + 1),
            clock: Some(at(n as i64 + 1)),
            kind: RunEventKind::Status {
                status: RunStatus::Running,
                message: None,
            },
        }],
        engine_checkpoint: vec![n as u8; 4],
        bot_state: Some(vec![9, n as u8]),
        undelivered: Some(vec![7, n as u8]),
    }
}

#[tokio::test]
async fn steps_survive_reopen_and_failed_commit_leaves_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    let store = StateStore::open(&path).await.unwrap();
    let run_id = store.create_run(new_run(Mode::Backtest)).await.unwrap();

    store
        .commit_step(step(
            run_id,
            1,
            vec![order(1, "a", OrderStatus::Filled, 10)],
        ))
        .await
        .unwrap();
    store
        .commit_step(step(
            run_id,
            2,
            vec![order(2, "b", OrderStatus::Filled, 10)],
        ))
        .await
        .unwrap();

    let duplicate_client = step(run_id, 3, vec![order(3, "a", OrderStatus::Working, 0)]);
    assert!(store.commit_step(duplicate_client).await.is_err());
    assert!(matches!(
        store.commit_step(step(run_id, 5, vec![])).await,
        Err(StateError::StaleStep { .. })
    ));

    let before = (
        store.ledger(run_id).await.unwrap(),
        store.portfolio(run_id).await.unwrap(),
        store.orders(run_id, None, 100).await.unwrap(),
        store.checkpoint(run_id).await.unwrap(),
        store.equity(run_id).await.unwrap(),
        store.events(run_id, None, 100, Vec::new()).await.unwrap(),
        store.fills(run_id, OrderId(1)..=OrderId(2)).await.unwrap(),
    );
    assert_eq!(before.0.len(), 2);
    assert_eq!(before.2.len(), 2);
    assert_eq!(before.3.as_ref().unwrap().step_seq, 2);
    assert_eq!(before.3.as_ref().unwrap().bot_state, Some(vec![9, 2]));
    assert_eq!(before.3.as_ref().unwrap().undelivered, Some(vec![7, 2]));
    assert_eq!(before.1.qty(&Symbol::new("AAPL")), 10);
    assert_eq!(before.6.len(), 1);
    assert_eq!(before.6[0].client_id, "b");

    drop(store);
    let reopened = StateStore::open(&path).await.unwrap();
    let after = (
        reopened.ledger(run_id).await.unwrap(),
        reopened.portfolio(run_id).await.unwrap(),
        reopened.orders(run_id, None, 100).await.unwrap(),
        reopened.checkpoint(run_id).await.unwrap(),
        reopened.equity(run_id).await.unwrap(),
        reopened
            .events(run_id, None, 100, Vec::new())
            .await
            .unwrap(),
        reopened
            .fills(run_id, OrderId(1)..=OrderId(2))
            .await
            .unwrap(),
    );
    assert_eq!(before, after);
    let summary = reopened.run(run_id).await.unwrap().unwrap();
    assert_eq!(summary.equity, Some(d("100002.25")));
}

#[tokio::test]
async fn a_bot_has_one_live_run() {
    let directory = tempfile::tempdir().unwrap();
    let store = StateStore::open(&directory.path().join(FILE_NAME))
        .await
        .unwrap();
    let first = store.create_run(new_run(Mode::Live)).await.unwrap();
    assert!(matches!(
        store.create_run(new_run(Mode::Live)).await,
        Err(StateError::LiveRunExists(_))
    ));
    store.create_run(new_run(Mode::Backtest)).await.unwrap();
    store
        .set_status(first, RunStatus::Cancelled, None, at(5))
        .await
        .unwrap();
    store.create_run(new_run(Mode::Live)).await.unwrap();
}

#[tokio::test]
async fn a_state_store_from_before_financing_and_undelivered_steps_takes_both() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE run (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                bot TEXT NOT NULL,
                mode TEXT NOT NULL CHECK (mode IN ('backtest', 'live')),
                status TEXT NOT NULL,
                desired TEXT NOT NULL,
                config TEXT NOT NULL,
                holdout INTEGER NOT NULL CHECK (holdout IN (0, 1)),
                bot_hash TEXT NOT NULL,
                engine_version TEXT NOT NULL,
                seed INTEGER NOT NULL,
                data_watermark INTEGER,
                created_at INTEGER NOT NULL,
                started_at INTEGER,
                finished_at INTEGER,
                step_seq INTEGER NOT NULL DEFAULT 0,
                clock INTEGER,
                equity TEXT,
                checkpoint BLOB,
                bot_state BLOB,
                result_hash TEXT,
                metrics TEXT,
                badges TEXT NOT NULL DEFAULT '[]',
                message TEXT
            ) STRICT;
            CREATE TABLE ledger_entry (
                run_id INTEGER NOT NULL REFERENCES run (id) ON DELETE CASCADE,
                seq INTEGER NOT NULL,
                at INTEGER NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('deposit', 'fill', 'dividend', 'split', 'borrow_fee', 'liquidation')),
                symbol TEXT,
                cash_delta TEXT NOT NULL,
                body TEXT NOT NULL,
                PRIMARY KEY (run_id, seq)
            ) STRICT;",
        )
        .unwrap();
    let store = StateStore::open(&path).await.unwrap();
    let run_id = store.create_run(new_run(Mode::Live)).await.unwrap();
    store.commit_step(step(run_id, 1, vec![])).await.unwrap();
    let mut financed = step(run_id, 2, vec![]);
    financed.ledger[0].kind = LedgerKind::Financing {
        balance: d("-5000"),
        rate: d("0.06"),
        amount: d("1.19"),
    };
    store.commit_step(financed).await.unwrap();

    let portfolio = store.portfolio(run_id).await.unwrap();
    assert_eq!(portfolio.cash, d("99998.81"));
    let checkpoint = store.checkpoint(run_id).await.unwrap().unwrap();
    assert_eq!(checkpoint.undelivered, Some(vec![7, 2]));
    let job = store.start_job("borrow", at(1)).await.unwrap();
    store
        .finish_job(job, at(1), JobOutcome::Waiting, 0, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_state_store_from_a_newer_spine_is_refused_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    drop(StateStore::open(&path).await.unwrap());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let latest: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    connection
        .pragma_update(None, "user_version", latest + 1)
        .unwrap();
    drop(connection);
    assert!(matches!(
        StateStore::open(&path).await,
        Err(StateError::NewerSchema { .. })
    ));
    let version: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, latest + 1);
}
