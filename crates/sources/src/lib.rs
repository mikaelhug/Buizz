pub mod alpaca;
pub mod alpaca_stream;
pub mod edgar;
pub mod fred;
pub mod http;
pub mod tiingo;
pub mod treasury;

use alpaca::AlpacaClient;
use edgar::EdgarClient;
use fred::FredClient;
use tiingo::TiingoClient;
use treasury::TreasuryClient;

#[derive(Clone)]
pub struct Sources {
    pub alpaca: Option<AlpacaClient>,
    pub tiingo: Option<TiingoClient>,
    pub edgar: Option<EdgarClient>,
    pub fred: Option<FredClient>,
    pub treasury: TreasuryClient,
}
