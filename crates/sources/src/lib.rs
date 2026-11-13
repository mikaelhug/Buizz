pub mod alpaca;
pub mod alpaca_stream;
pub mod anthropic;
pub mod cboe;
pub mod edgar;
pub mod factset;
pub mod fed;
pub mod finra;
pub mod fred;
pub mod http;
pub mod ibkr;
pub mod nasdaq;
pub mod tiingo;
pub mod treasury;
pub mod wikipedia;

use alpaca::AlpacaClient;
use anthropic::AnthropicClient;
use cboe::CboeClient;
use edgar::EdgarClient;
use factset::FactsetClient;
use fed::FedClient;
use finra::FinraClient;
use fred::FredClient;
use ibkr::IbkrClient;
use nasdaq::NasdaqClient;
use tiingo::TiingoClient;
use treasury::TreasuryClient;
use wikipedia::WikipediaClient;

#[derive(Clone)]
pub struct Sources {
    pub alpaca: Option<AlpacaClient>,
    pub tiingo: Option<TiingoClient>,
    pub edgar: Option<EdgarClient>,
    pub fred: Option<FredClient>,
    pub treasury: TreasuryClient,
    pub wikipedia: Option<WikipediaClient>,
    pub ibkr: Option<IbkrClient>,
    pub nasdaq: Option<NasdaqClient>,
    pub cboe: Option<CboeClient>,
    pub fed: Option<FedClient>,
    pub factset: Option<FactsetClient>,
    pub finra: Option<FinraClient>,
    pub anthropic: Option<AnthropicClient>,
}
