use buizz_core::market::Symbol;
use buizz_sources::ibkr::normalize_borrow;
use rust_decimal::Decimal;

const FIXTURE: &[u8] = include_bytes!("../fixtures/ibkr/usa.txt");

#[test]
fn the_short_stock_file_becomes_one_record_per_us_ticker_stamped_with_its_new_york_time() {
    let records = normalize_borrow(FIXTURE).unwrap();

    let symbols: Vec<&str> = records
        .iter()
        .map(|record| record.symbol.as_str())
        .collect();
    assert_eq!(symbols, ["AAPL", "ABEW", "BF.B", "GME", "SPY", "2777456Q"]);
    assert!(
        records
            .iter()
            .all(|record| record.at.to_string() == "2026-11-11T20:10:41Z")
    );
    let apple = &records[0];
    assert_eq!(
        (apple.available, apple.available_or_more),
        (10_000_000, true)
    );
    let gamestop = records
        .iter()
        .find(|record| record.symbol == Symbol::new("GME"))
        .unwrap();
    assert_eq!(gamestop.fee_rate, Some(Decimal::new(4115, 4)));
    assert_eq!(gamestop.rebate_rate, Some(Decimal::new(34685, 4)));
    assert_eq!(
        (gamestop.available, gamestop.available_or_more),
        (7_600_000, false)
    );
    assert_eq!(records[5].fee_rate, None);

    let truncated = &FIXTURE[..FIXTURE.len() - 10];
    assert!(
        normalize_borrow(truncated).is_err(),
        "a cut-off download is refused"
    );
}
