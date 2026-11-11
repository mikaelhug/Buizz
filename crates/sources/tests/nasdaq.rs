use buizz_core::market::Symbol;
use buizz_sources::nasdaq::normalize_etf_symbols;

const DIRECTORY: &str = "\
Nasdaq Traded|Symbol|Security Name|Listing Exchange|Market Category|ETF|Round Lot Size|Test Issue|Financial Status|CQS Symbol|NASDAQ Symbol|NextShares
Y|AAPL|Apple Inc. - Common Stock|Q|Q|N|40|N|N||AAPL|N
Y|GLD|SPDR Gold Shares|P| |Y|40|N||GLD|GLD|N
Y|SPY|State Street SPDR S&P 500 ETF Trust|P| |Y|40|N||SPY|SPY|N
Y|ZXIET|NASDAQ TEST STOCK|Q|G|Y|100|Y|N||ZXIET|N
File Creation Time: 1111202618:02|||||
";

#[test]
fn trusts_the_sec_fund_list_misses_are_etfs_and_test_issues_are_not() {
    let etfs = normalize_etf_symbols(DIRECTORY).unwrap();
    assert_eq!(
        etfs.into_iter().collect::<Vec<_>>(),
        [Symbol::new("GLD"), Symbol::new("SPY")]
    );
    let cut = DIRECTORY.replace("File Creation Time: 1111202618:02|||||\n", "");
    assert!(normalize_etf_symbols(&cut).is_err());
}
