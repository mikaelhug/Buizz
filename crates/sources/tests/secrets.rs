use buizz_sources::alpaca::AlpacaConfig;
use buizz_sources::alpaca_stream::{StreamConfig, stream_keys};
use buizz_sources::tiingo::TiingoConfig;

#[test]
fn debug_output_never_shows_api_secrets() {
    let alpaca = AlpacaConfig::new("key-id".to_owned(), "alpaca-secret".to_owned());
    let stream = format!("{:?}", StreamConfig::new(alpaca.clone()));
    let tiingo = format!("{:?}", TiingoConfig::new("tiingo-secret".to_owned()));
    let accounts = stream_keys("first:alpaca-secret\n\n second : alpaca-secret-2 \n")
        .expect("two stream accounts");
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[1].key_id, "second");
    let malformed = stream_keys("first:alpaca-secret\nalpaca-secret-without-key-id")
        .expect_err("a line without a key id is refused");
    let repeated = stream_keys("first:alpaca-secret\nfirst:alpaca-secret-2")
        .expect_err("one stream per account");
    for printed in [
        format!("{alpaca:?}"),
        stream,
        tiingo,
        format!("{accounts:?}"),
        format!("{malformed} {malformed:?}"),
        format!("{repeated} {repeated:?}"),
    ] {
        assert!(!printed.contains("alpaca-secret"), "{printed}");
        assert!(!printed.contains("tiingo-secret"), "{printed}");
        assert!(
            printed.contains("<redacted>") || printed.contains("ALPACA_STREAM_KEYS"),
            "{printed}"
        );
    }
}
