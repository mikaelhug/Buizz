use std::collections::{BTreeMap, BTreeSet, HashMap};

use buizz_core::market::Symbol;
use buizz_core::performance::{Cadence, DrawdownResponse, Fingerprint};
use chrono::{Datelike, NaiveDate};

use crate::mean;
use crate::research::median;

pub const PRICE_MOVE_REASON: &str = "price:";
pub const WEAK_STOCK_DRAWDOWN: f64 = 0.20;
pub const WEAK_MARKET_DRAWDOWN: f64 = 0.10;

const TOP_NAMES: usize = 5;
const DAYS_PER_YEAR: f64 = 365.25;
const MONTHS_PER_YEAR: f64 = 12.0;
const MONTHS_PER_QUARTER: u32 = 3;

pub struct Drawdown {
    pub label: &'static str,
    pub from: NaiveDate,
    pub to: NaiveDate,
}

pub fn market_drawdowns() -> [Drawdown; 2] {
    let day = |year, month, day| NaiveDate::from_ymd_opt(year, month, day).expect("a valid date");
    [
        Drawdown {
            label: "COVID crash",
            from: day(2020, 2, 19),
            to: day(2020, 3, 23),
        },
        Drawdown {
            label: "2022 bear market",
            from: day(2022, 1, 3),
            to: day(2022, 10, 12),
        },
    ]
}

pub struct Book {
    pub date: NaiveDate,
    pub equity: Option<f64>,
    pub stocks: BTreeMap<Symbol, f64>,
    pub beta: Option<f64>,
}

impl Book {
    fn stock_value(&self) -> f64 {
        self.stocks.values().fold(0.0, |total, value| total + value)
    }
}

pub struct Trade {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub value: f64,
    pub stock: bool,
    pub weak: Option<bool>,
    pub on_price: Option<bool>,
}

impl Trade {
    fn overlaps(&self, window: &Drawdown) -> bool {
        self.from <= window.to && window.from <= self.to
    }
}

pub fn drawdown_from_high(closes: &[f64]) -> Option<f64> {
    let last = *closes.last()?;
    let high = closes.iter().copied().fold(f64::MIN, f64::max);
    (high > 0.0).then(|| 1.0 - last / high)
}

pub fn fingerprint(cadence: Cadence, books: &[Book], trades: &[Trade]) -> Option<Fingerprint> {
    let (first, last) = (books.first()?, books.last()?);
    let held: Vec<&Book> = books
        .iter()
        .filter(|book| !book.stocks.is_empty())
        .collect();
    let counts: Vec<u32> = books
        .iter()
        .map(|book| u32::try_from(book.stocks.len()).unwrap_or(u32::MAX))
        .collect();
    let spans = holding_months(books);
    let stock_trades: Vec<&Trade> = trades.iter().filter(|trade| trade.stock).collect();
    let buys = stock_trades.iter().filter(|trade| trade.value > 0.0);
    let sells = stock_trades.iter().filter(|trade| trade.value < 0.0);
    Some(Fingerprint {
        cadence,
        from: first.date,
        to: last.date,
        observations: u32::try_from(books.len()).unwrap_or(u32::MAX),
        positions_median: median(counts.iter().map(|count| f64::from(*count)).collect()),
        positions_min: counts.iter().min().copied(),
        positions_max: counts.iter().max().copied(),
        top_five_share: average(held.iter().map(|book| top_share(book))),
        turnover: turnover(books, &stock_trades),
        holding_months_median: median(spans.clone()),
        holding_months_mean: average(spans.iter().copied()),
        holdings: u32::try_from(spans.len()).unwrap_or(u32::MAX),
        stock_share: average(books.iter().filter_map(|book| {
            book.equity
                .filter(|equity| *equity > 0.0)
                .map(|equity| book.stock_value() / equity)
        })),
        idle_months: (cadence == Cadence::Daily)
            .then(|| idle_share(cadence, books, trades, 1))
            .flatten(),
        idle_quarters: idle_share(cadence, books, trades, MONTHS_PER_QUARTER),
        buys_into_weakness: dollar_share(buys, |trade| trade.weak),
        sells_on_price: dollar_share(sells, |trade| trade.on_price),
        drawdowns: market_drawdowns()
            .into_iter()
            .map(|window| DrawdownResponse {
                net_buying: net_buying(books, &stock_trades, &window),
                label: window.label.to_owned(),
                from: window.from,
                to: window.to,
            })
            .collect(),
        beta: average(books.iter().filter_map(|book| book.beta)),
    })
}

fn average(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values: Vec<f64> = values.filter(|value| value.is_finite()).collect();
    (!values.is_empty()).then(|| mean(&values))
}

fn top_share(book: &Book) -> f64 {
    let mut values: Vec<f64> = book.stocks.values().copied().collect();
    values.sort_by(|left, right| right.total_cmp(left));
    let total: f64 = values.iter().sum();
    values.iter().take(TOP_NAMES).sum::<f64>() / total
}

fn holding_months(books: &[Book]) -> Vec<f64> {
    let months = |from: NaiveDate, to: NaiveDate| {
        (to - from).num_days() as f64 * MONTHS_PER_YEAR / DAYS_PER_YEAR
    };
    let mut open: HashMap<&Symbol, NaiveDate> = HashMap::new();
    let mut spans = Vec::new();
    for book in books {
        open.retain(|symbol, since| {
            let kept = book.stocks.contains_key(*symbol);
            if !kept {
                spans.push(months(*since, book.date));
            }
            kept
        });
        for symbol in book.stocks.keys() {
            open.entry(symbol).or_insert(book.date);
        }
    }
    let end = books.last().map(|book| book.date);
    spans.extend(
        end.into_iter()
            .flat_map(|end| open.values().map(move |since| months(*since, end))),
    );
    spans
}

fn turnover(books: &[Book], trades: &[&Trade]) -> Option<f64> {
    let (first, last) = (books.first()?, books.last()?);
    let years = (last.date - first.date).num_days() as f64 / DAYS_PER_YEAR;
    let book = average(books.iter().map(Book::stock_value)).filter(|book| *book > 0.0)?;
    let purchases: f64 = trades.iter().map(|trade| trade.value.max(0.0)).sum();
    let sales: f64 = trades.iter().map(|trade| (-trade.value).max(0.0)).sum();
    (years > 0.0).then(|| purchases.min(sales) / book / years)
}

fn period(date: NaiveDate, months: u32) -> (i32, u32) {
    (date.year(), date.month0() / months)
}

fn idle_share(cadence: Cadence, books: &[Book], trades: &[Trade], months: u32) -> Option<f64> {
    let (first, last) = (books.first()?.date, books.last()?.date);
    let observed: BTreeSet<(i32, u32)> = match cadence {
        Cadence::Quarterly => books
            .iter()
            .skip(1)
            .map(|book| period(book.date, months))
            .collect(),
        Cadence::Daily => first
            .iter_days()
            .take_while(|day| *day <= last)
            .map(|day| period(day, months))
            .collect(),
    };
    let active: BTreeSet<(i32, u32)> = trades
        .iter()
        .map(|trade| period(trade.to, months))
        .collect();
    let idle = observed.iter().filter(|key| !active.contains(key)).count();
    (!observed.is_empty()).then(|| idle as f64 / observed.len() as f64)
}

fn dollar_share<'a>(
    trades: impl Iterator<Item = &'a &'a Trade>,
    flag: impl Fn(&Trade) -> Option<bool>,
) -> Option<f64> {
    let (flagged, known) = trades.fold((0.0, 0.0), |(flagged, known), trade| match flag(trade) {
        Some(true) => (flagged + trade.value.abs(), known + trade.value.abs()),
        Some(false) => (flagged, known + trade.value.abs()),
        None => (flagged, known),
    });
    (known > 0.0).then(|| flagged / known)
}

fn net_buying(books: &[Book], trades: &[&Trade], window: &Drawdown) -> Option<f64> {
    let before = books.iter().rev().find(|book| book.date < window.from)?;
    books.iter().find(|book| book.date >= window.to)?;
    let base = before.equity.unwrap_or_else(|| before.stock_value());
    let net: f64 = trades
        .iter()
        .filter(|trade| trade.overlaps(window))
        .map(|trade| trade.value)
        .sum();
    (base > 0.0).then(|| net / base)
}
