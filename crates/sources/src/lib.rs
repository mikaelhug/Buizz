pub mod alpaca;
pub mod alpaca_stream;
pub mod edgar;
pub mod http;
pub mod ibkr;
pub mod tiingo;
pub mod treasury;
pub mod wikipedia;

use alpaca::AlpacaClient;
use edgar::EdgarClient;
use ibkr::IbkrClient;
use tiingo::TiingoClient;
use treasury::TreasuryClient;
use wikipedia::WikipediaClient;

#[derive(Clone)]
pub struct Sources {
    pub alpaca: Option<AlpacaClient>,
    pub tiingo: Option<TiingoClient>,
    pub edgar: Option<EdgarClient>,
    pub treasury: TreasuryClient,
    pub wikipedia: Option<WikipediaClient>,
    pub ibkr: Option<IbkrClient>,
}
