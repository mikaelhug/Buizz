use buizz_sources::alpaca::AlpacaConfig;
use buizz_sources::alpaca_stream::StreamConfig;
use buizz_sources::tiingo::TiingoConfig;

#[test]
fn debug_output_never_shows_api_secrets() {
    let alpaca = AlpacaConfig::new("key-id".to_owned(), "alpaca-secret".to_owned());
    let stream = format!("{:?}", StreamConfig::new(alpaca.clone()));
    let tiingo = format!("{:?}", TiingoConfig::new("tiingo-secret".to_owned()));
    for printed in [format!("{alpaca:?}"), stream, tiingo] {
        assert!(!printed.contains("alpaca-secret"), "{printed}");
        assert!(!printed.contains("tiingo-secret"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }
}
