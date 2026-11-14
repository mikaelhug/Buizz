use std::path::Path;

use buizz_core::market::{
    Adjustment, Bar, CorporateAction, CorporateActionKind, DataSource, Symbol, Timeframe,
};
use buizz_core::protocol::BarRow;
use buizz_market::{ActionWindow, FILE_NAME, HistoryRequest, MarketStore};
use chrono::NaiveDate;
use rust_decimal::Decimal;

const DIVIDEND_PAYERS: usize = 5_000;
const EX_DATES: [(u32, u32); 4] = [(11, 25), (11, 26), (11, 27), (12, 2)];

async fn adjusted_history(store: &MarketStore, symbols: Vec<Symbol>) -> Vec<BarRow> {
    store
        .history(
            HistoryRequest {
                symbols,
                timeframe: Timeframe::Day,
                bars: 10,
                adjust: Adjustment::All,
                as_of: "2024-12-06T21:00:00Z".parse().unwrap(),
            },
            None,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn adjusting_a_decade_of_index_dividends_keeps_the_reader_alive() {
    let dir = tempfile::tempdir().unwrap();
    let store = MarketStore::open(&dir.path().join(FILE_NAME), 1)
        .await
        .unwrap();
    store
        .load_fixture(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/market"))
        .await
        .unwrap();
    let ex_dates: Vec<NaiveDate> = EX_DATES
        .iter()
        .map(|(month, day)| NaiveDate::from_ymd_opt(2024, *month, *day).unwrap())
        .collect();
    let payers: Vec<Symbol> = (0..DIVIDEND_PAYERS)
        .map(|index| Symbol::new(&format!("D{index:04}")))
        .collect();
    let template = store
        .bars_between(
            Timeframe::Day,
            vec![Symbol::new("AAA")],
            "2024-11-01T00:00:00Z".parse().unwrap(),
            "2024-12-07T00:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
    store
        .upsert_bars(
            DataSource::Fixture,
            Timeframe::Day,
            payers
                .iter()
                .flat_map(|symbol| {
                    template.iter().map(|bar| Bar {
                        symbol: symbol.clone(),
                        ..bar.clone()
                    })
                })
                .collect(),
        )
        .await
        .unwrap();
    store
        .replace_corporate_actions(
            DataSource::Fixture,
            payers
                .iter()
                .flat_map(|symbol| {
                    ex_dates.iter().map(|ex_date| CorporateAction {
                        symbol: symbol.clone(),
                        ex_date: *ex_date,
                        kind: CorporateActionKind::CashDividend { rate: Decimal::ONE },
                    })
                })
                .collect(),
            ActionWindow {
                symbols: payers.clone(),
                from: ex_dates[0],
                through: ex_dates[ex_dates.len() - 1],
                covered_to: Some(ex_dates[ex_dates.len() - 1]),
            },
        )
        .await
        .unwrap();

    let alone = adjusted_history(&store, vec![Symbol::new("BBB")]).await;
    let wide = adjusted_history(&store, [vec![Symbol::new("BBB")], payers].concat()).await;

    assert_eq!(
        wide.into_iter()
            .filter(|row| row.symbol == Symbol::new("BBB"))
            .collect::<Vec<_>>(),
        alone
    );
}
