use std::collections::{BTreeMap, BTreeSet, HashMap};

use buizz_core::api::RunSummary;
use buizz_core::ledger::{LedgerKind, Portfolio};
use buizz_core::market::{Adjustment, AdjustmentFactor, AssetClass, Symbol, Timeframe};
use buizz_core::order::OrderId;
use buizz_core::performance::{Behaviour, Cadence, ReferenceFingerprint};
use buizz_core::time::Timestamp;
use buizz_market::MarketStore;
use buizz_metrics::{
    Book, PRICE_MOVE_REASON, Trade, WEAK_MARKET_DRAWDOWN, WEAK_STOCK_DRAWDOWN, beta,
    drawdown_from_high, fingerprint,
};
use buizz_state::StateStore;
use chrono::{Datelike, Days, NaiveDate, NaiveTime};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::RunnerError;

const BERKSHIRE_13F: &str = include_str!("../reference/berkshire_13f.csv");
const BERKSHIRE_NAME: &str = "Berkshire Hathaway 13F";
const BERKSHIRE_SOURCE: &str = "SEC EDGAR 13F-HR information tables, CIK 1067983";
const YEAR_SESSIONS: usize = 252;
const MIN_BETA_RETURNS: usize = 120;
const LOOKBACK: Days = Days::new(400);

type Holdings = BTreeMap<Symbol, (f64, f64)>;

struct Prices {
    raw: HashMap<Symbol, BTreeMap<NaiveDate, f64>>,
    adjusted: HashMap<Symbol, BTreeMap<NaiveDate, f64>>,
    splits: HashMap<Symbol, Vec<AdjustmentFactor>>,
    market: Symbol,
}

impl Prices {
    async fn load(
        market: &MarketStore,
        mut symbols: BTreeSet<Symbol>,
        benchmark: &Symbol,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Self, RunnerError> {
        symbols.insert(benchmark.clone());
        let symbols: Vec<Symbol> = symbols.into_iter().collect();
        let first = from - LOOKBACK;
        let at = |date| Timestamp::at_exchange_time(date, NaiveTime::MIN);
        let (Some(start), Some(end)) = (at(first), at(to + Days::new(1))) else {
            return Ok(Self::empty(benchmark));
        };
        let bars = market
            .traded_bars(Timeframe::Day, symbols.clone(), start, end)
            .await?;
        let mut splits: HashMap<Symbol, Vec<AdjustmentFactor>> = HashMap::new();
        for action in market.traded_actions(symbols, first, to).await? {
            if let Some(factor) = AdjustmentFactor::for_shares(&action, Adjustment::Split) {
                splits.entry(action.symbol).or_default().push(factor);
            }
        }
        let mut raw: HashMap<Symbol, BTreeMap<NaiveDate, f64>> = HashMap::new();
        let mut adjusted: HashMap<Symbol, BTreeMap<NaiveDate, f64>> = HashMap::new();
        for bar in bars {
            let date = bar.start.exchange_date();
            let factor = splits.get(&bar.symbol).map_or(Decimal::ONE, |factors| {
                AdjustmentFactor::cumulative(date, factors).0
            });
            let (Some(close), Some(scaled)) = (bar.close.to_f64(), (bar.close * factor).to_f64())
            else {
                continue;
            };
            raw.entry(bar.symbol.clone())
                .or_default()
                .insert(date, close);
            adjusted.entry(bar.symbol).or_default().insert(date, scaled);
        }
        Ok(Self {
            raw,
            adjusted,
            splits,
            market: benchmark.clone(),
        })
    }

    fn empty(benchmark: &Symbol) -> Self {
        Self {
            raw: HashMap::new(),
            adjusted: HashMap::new(),
            splits: HashMap::new(),
            market: benchmark.clone(),
        }
    }

    fn close(&self, symbol: &Symbol, date: NaiveDate) -> Option<f64> {
        self.raw
            .get(symbol)?
            .range(..=date)
            .next_back()
            .map(|(_, close)| *close)
    }

    fn year_to(&self, symbol: &Symbol, date: NaiveDate) -> Vec<(NaiveDate, f64)> {
        let Some(series) = self.adjusted.get(symbol) else {
            return Vec::new();
        };
        let mut year: Vec<(NaiveDate, f64)> = series
            .range(..=date)
            .rev()
            .take(YEAR_SESSIONS + 1)
            .map(|(day, close)| (*day, *close))
            .collect();
        year.reverse();
        year
    }

    fn below_high(&self, symbol: &Symbol, date: NaiveDate) -> Option<f64> {
        let closes: Vec<f64> = self
            .year_to(symbol, date)
            .into_iter()
            .map(|(_, close)| close)
            .collect();
        drawdown_from_high(&closes)
    }

    fn weak(&self, symbol: &Symbol, date: NaiveDate) -> Option<bool> {
        let stock = self.below_high(symbol, date);
        let market = self.below_high(&self.market, date);
        let breached = stock.is_some_and(|depth| depth >= WEAK_STOCK_DRAWDOWN)
            || market.is_some_and(|depth| depth >= WEAK_MARKET_DRAWDOWN);
        (breached || (stock.is_some() && market.is_some())).then_some(breached)
    }

    fn beta(&self, symbol: &Symbol, date: NaiveDate) -> Option<f64> {
        let market: BTreeMap<NaiveDate, f64> =
            self.year_to(&self.market, date).into_iter().collect();
        let paired: Vec<(f64, f64)> = self
            .year_to(symbol, date)
            .into_iter()
            .filter_map(|(day, close)| Some((close, *market.get(&day)?)))
            .collect();
        let (stock, index): (Vec<f64>, Vec<f64>) = paired
            .windows(2)
            .map(|pair| (pair[1].0 / pair[0].0 - 1.0, pair[1].1 / pair[0].1 - 1.0))
            .unzip();
        (stock.len() >= MIN_BETA_RETURNS)
            .then(|| beta(&stock, &index))
            .flatten()
    }

    fn book_beta(&self, stocks: &BTreeMap<Symbol, f64>, date: NaiveDate) -> Option<f64> {
        let (weighted, weight) = stocks
            .iter()
            .filter_map(|(symbol, value)| Some((self.beta(symbol, date)?, *value)))
            .fold((0.0, 0.0), |(sum, total), (beta, value)| {
                (sum + beta * value, total + value)
            });
        (weight > 0.0).then(|| weighted / weight)
    }

    fn share_multiplier(&self, symbol: &Symbol, after: NaiveDate, through: NaiveDate) -> f64 {
        self.splits
            .get(symbol)
            .into_iter()
            .flatten()
            .filter(|factor| factor.ex_date > after && factor.ex_date <= through)
            .filter_map(|factor| factor.volume.to_f64())
            .product()
    }
}

struct Stocks(HashMap<Symbol, AssetClass>);

impl Stocks {
    async fn load(market: &MarketStore) -> Result<Self, RunnerError> {
        Ok(Self(
            market
                .instruments()
                .await?
                .into_iter()
                .map(|instrument| (instrument.symbol, instrument.asset_class))
                .collect(),
        ))
    }

    fn contains(&self, symbol: &Symbol) -> bool {
        self.0.get(symbol) != Some(&AssetClass::Etf)
    }
}

fn quarter_ends(dates: &[NaiveDate]) -> BTreeSet<NaiveDate> {
    let quarter = |date: &NaiveDate| (date.year(), date.month0() / 3);
    dates
        .windows(2)
        .filter(|pair| quarter(&pair[0]) != quarter(&pair[1]))
        .map(|pair| pair[0])
        .chain(dates.last().copied())
        .collect()
}

fn berkshire() -> BTreeMap<NaiveDate, Holdings> {
    let mut quarters: BTreeMap<NaiveDate, Holdings> = BTreeMap::new();
    for line in BERKSHIRE_13F.lines().skip(1) {
        let fields: Vec<&str> = line.splitn(7, ',').collect();
        let [report, _, _, ticker, shares, value, _] = fields[..] else {
            continue;
        };
        let (Ok(report), Ok(shares), Ok(value)) = (
            report.parse::<NaiveDate>(),
            shares.parse::<f64>(),
            value.parse::<f64>(),
        ) else {
            continue;
        };
        let held = quarters
            .entry(report)
            .or_default()
            .entry(Symbol::new(ticker))
            .or_default();
        held.0 += shares;
        held.1 += value;
    }
    quarters
}

struct RunRecord {
    dates: Vec<(NaiveDate, Timestamp, f64)>,
    ledger: Vec<buizz_core::ledger::LedgerEntry>,
    reasons: HashMap<OrderId, String>,
}

impl RunRecord {
    fn symbols(&self) -> BTreeSet<Symbol> {
        self.ledger
            .iter()
            .filter_map(|entry| entry.kind.symbol().cloned())
            .collect()
    }

    fn fingerprint(
        &self,
        prices: &Prices,
        stocks: &Stocks,
    ) -> Option<buizz_core::performance::Fingerprint> {
        let ends = quarter_ends(
            &self
                .dates
                .iter()
                .map(|(date, ..)| *date)
                .collect::<Vec<_>>(),
        );
        let mut portfolio = Portfolio::default();
        let mut entries = self.ledger.iter().peekable();
        let mut books = Vec::with_capacity(self.dates.len());
        for (date, at, equity) in &self.dates {
            while let Some(entry) = entries.next_if(|entry| entry.at <= *at) {
                portfolio.apply(entry.at, &entry.kind);
            }
            let held: BTreeMap<Symbol, f64> = portfolio
                .open_positions()
                .filter(|(symbol, position)| position.qty > 0 && stocks.contains(symbol))
                .filter_map(|(symbol, position)| {
                    let price = prices
                        .close(symbol, *date)
                        .or_else(|| position.last_price.to_f64())?;
                    Some((symbol.clone(), position.qty as f64 * price))
                })
                .collect();
            books.push(Book {
                date: *date,
                equity: Some(*equity),
                beta: ends
                    .contains(date)
                    .then(|| prices.book_beta(&held, *date))
                    .flatten(),
                stocks: held,
            });
        }
        let trades: Vec<Trade> = self
            .ledger
            .iter()
            .filter_map(|entry| {
                let (symbol, qty, price, on_price) = match &entry.kind {
                    LedgerKind::Fill {
                        order_id,
                        symbol,
                        qty,
                        price,
                        ..
                    } => (
                        symbol,
                        qty,
                        price,
                        self.reasons
                            .get(order_id)
                            .map(|reason| reason.starts_with(PRICE_MOVE_REASON)),
                    ),
                    LedgerKind::Liquidation { symbol, qty, price } => {
                        (symbol, qty, price, Some(true))
                    }
                    _ => return None,
                };
                let date = entry.at.exchange_date();
                let value = (Decimal::from(*qty) * *price).to_f64()?;
                Some(Trade {
                    from: date,
                    to: date,
                    value,
                    stock: stocks.contains(symbol),
                    weak: (value > 0.0).then(|| prices.weak(symbol, date)).flatten(),
                    on_price: (value < 0.0).then_some(on_price).flatten(),
                })
            })
            .collect();
        fingerprint(Cadence::Daily, &books, &trades)
    }
}

fn reference_fingerprint(
    quarters: &BTreeMap<NaiveDate, Holdings>,
    prices: &Prices,
    stocks: &Stocks,
) -> Option<buizz_core::performance::Fingerprint> {
    let books: Vec<Book> = quarters
        .iter()
        .map(|(date, holdings)| {
            let held: BTreeMap<Symbol, f64> = holdings
                .iter()
                .filter(|(symbol, (shares, _))| *shares > 0.0 && stocks.contains(symbol))
                .map(|(symbol, (_, value))| (symbol.clone(), *value))
                .collect();
            Book {
                date: *date,
                equity: None,
                beta: prices.book_beta(&held, *date),
                stocks: held,
            }
        })
        .collect();
    let mut trades = Vec::new();
    for ((before, earlier), (now, later)) in quarters.iter().zip(quarters.iter().skip(1)) {
        let symbols: BTreeSet<&Symbol> = earlier.keys().chain(later.keys()).collect();
        for symbol in symbols {
            let multiplier = prices.share_multiplier(symbol, *before, *now);
            let (had, _) = earlier.get(symbol).copied().unwrap_or_default();
            let (has, value) = later.get(symbol).copied().unwrap_or_default();
            let change = has - had * multiplier;
            let price = if has > 0.0 {
                Some(value / has)
            } else {
                prices.close(symbol, *now).or_else(|| {
                    earlier
                        .get(symbol)
                        .map(|(shares, value)| value / shares / multiplier)
                })
            };
            let Some(price) = price.filter(|_| change != 0.0) else {
                continue;
            };
            let value = change * price;
            trades.push(Trade {
                from: before.succ_opt().unwrap_or(*before),
                to: *now,
                value,
                stock: stocks.contains(symbol),
                weak: (value > 0.0).then(|| prices.weak(symbol, *now)).flatten(),
                on_price: None,
            });
        }
    }
    fingerprint(Cadence::Quarterly, &books, &trades)
}

pub(crate) async fn behaviour(
    market: &MarketStore,
    state: &StateStore,
    run: &RunSummary,
    today: NaiveDate,
) -> Result<Behaviour, RunnerError> {
    let mut dates: Vec<(NaiveDate, Timestamp, f64)> = Vec::new();
    for point in state.equity(run.id).await? {
        let date = point.at.exchange_date();
        let equity = point.equity.to_f64().unwrap_or(f64::NAN);
        match dates.last_mut() {
            Some(last) if last.0 == date => *last = (date, point.at, equity),
            _ => dates.push((date, point.at, equity)),
        }
    }
    let record = RunRecord {
        dates,
        ledger: state.ledger(run.id).await?,
        reasons: state
            .orders(run.id, None, u32::MAX)
            .await?
            .into_iter()
            .map(|order| (order.id, order.reason))
            .collect(),
    };
    let (from, to) = (run.config.start, run.config.end.unwrap_or(today));
    let all = berkshire();
    let opening = all.range(..=from).next_back().map(|(date, _)| *date);
    let quarters: BTreeMap<NaiveDate, Holdings> = all
        .into_iter()
        .filter(|(date, _)| opening.is_none_or(|opening| *date >= opening) && *date <= to)
        .collect();
    let symbols: BTreeSet<Symbol> = record
        .symbols()
        .into_iter()
        .chain(
            quarters
                .values()
                .flat_map(|holdings| holdings.keys().cloned()),
        )
        .collect();
    let stocks = Stocks::load(market).await?;
    let prices = Prices::load(market, symbols, &run.config.benchmark, from, to).await?;
    let references = reference_fingerprint(&quarters, &prices, &stocks)
        .filter(|reference| reference.observations > 1)
        .map(|fingerprint| ReferenceFingerprint {
            name: BERKSHIRE_NAME.to_owned(),
            source: BERKSHIRE_SOURCE.to_owned(),
            fingerprint,
        });
    Ok(Behaviour {
        run: record.fingerprint(&prices, &stocks),
        references: references.into_iter().collect(),
    })
}
