use std::collections::BTreeMap;
use std::sync::LazyLock;

use buizz_core::market::{
    EarningsInsight, EstimateFact, EstimateMetric, EstimatePeriod, EstimateScope,
};
use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};
use regex::{Captures, Regex};
use rust_decimal::Decimal;

const NUMBER: &str = r"(-?\d[\d,]*(?:\.\d+)?)";
const FORWARD: &str = r"(?:forward 12-month|12-month forward) P/E ratio";
const QUARTER: &str = r"Q([1-4]) (\d{4})";
const NAME: &str = r"(?-i:[A-Z][a-z]+(?: [A-Z][a-z]+)?)";
const SECTORS: [(&str, EstimateScope); 13] = [
    (
        "Consumer Discretionary",
        EstimateScope::ConsumerDiscretionary,
    ),
    ("Consumer Staples", EstimateScope::ConsumerStaples),
    ("Energy", EstimateScope::Energy),
    ("Financials", EstimateScope::Financials),
    ("Health Care", EstimateScope::HealthCare),
    ("Industrials", EstimateScope::Industrials),
    (
        "Information Technology",
        EstimateScope::InformationTechnology,
    ),
    ("Materials", EstimateScope::Materials),
    ("Telecom Services", EstimateScope::TelecomServices),
    ("Telecommunication Services", EstimateScope::TelecomServices),
    ("Utilities", EstimateScope::Utilities),
    ("Real Estate", EstimateScope::RealEstate),
    (
        "Communication Services",
        EstimateScope::CommunicationServices,
    ),
];

fn pattern(text: &str) -> Regex {
    let text = text
        .replace("{N}", NUMBER)
        .replace("{FPE}", FORWARD)
        .replace("{Q}", QUARTER)
        .replace("{NAME}", NAME);
    Regex::new(&format!("(?i){text}")).expect("valid pattern")
}

macro_rules! patterns {
    ($($name:ident = $text:expr;)*) => {
        $(static $name: LazyLock<Regex> = LazyLock::new(|| pattern($text));)*
    };
}

patterns! {
    STATED_PE = r"{FPE} (?:for the (?:S&P 500|index) )?is (?:now )?(?:at )?{N}";
    CURRENT_PE = r"current {FPE} of {N} is";
    INDEX_PE = r"P/E ratio of {N} for the index as a whole";
    DATED_PE = r"On (January|February|March|April|May|June|July|August|September|October|November|December) (\d{1,2}), the {FPE} for the S&P 500 was {N}";
    BASIS = r"based on (?:(Monday|Tuesday|Wednesday|Thursday|Friday|yesterday)'s )?(?:\([^)]*\) )?(?:a )?closing price (?:of |\(){N}\)? and (?:a |the )?forward 12-month EPS (?:estimate )?(?:of |\()\$?{N}";
    AVERAGE = r"\b(5|five|10|ten)-year average(?: {FPE}| P/E ratio)? (?:of |\(){N}";
    PAST_AVERAGE = r"average {FPE} over the past (5|five|10|ten) years \({N}\)";
    BRACKETED_AVERAGE = r"\b(5|five|10|ten)-year \({N}\) average";
    TRAILING_PE = r"trailing 12-month P/E ratio (?:for the S&P 500 )?is {N}";
    CURRENT_GROWTH = r"(?:For {Q}, )?the (?:estimated|blended)(?: \(year-over-year\))? (earnings|revenue) (growth rate|decline) (?:for (?:the S&P 500|{Q}) )?is (?:now )?{N}%";
    DECLINES = r"declines? in (?:both )?earnings \({N}%\) and revenues? \({N}%\)";
    PROJECTED_GROWTH = r"For (?:{Q}|CY (\d{4})), analysts are (?:projecting|predicting|expecting|calling for)(?: \(year-over-year\))? earnings growth (?:rate )?of {N}% and revenue growth (?:rate )?of {N}%";
    EPS_BEATS = r"{N}% (?:of (?:S&P 500 )?companies )?ha(?:ve|s) reported (?:(?:actual )?(?:EPS|earnings) above (?:the mean )?estimates?\b|a positive EPS surprise)";
    REVENUE_BEATS = r"{N}% (?:of (?:S&P 500 )?companies )?ha(?:ve|s) reported (?:(?:actual )?(?:sales|revenues?) above (?:the mean )?estimates?\b|a positive (?:revenue|sales) surprise)";
    SENTENCE_QUARTER = r"\bfor {Q}\b";
    GUIDANCE = r"For {Q}, {N} (?:S&P 500 )?companies have issued negative EPS guidance and {N} (?:S&P 500 )?companies have issued positive EPS guidance";
    TARGET = r"bottom-up target price for the S&P 500 is {N}, which is {N}% (above|below) the closing price of {N}";
    RATINGS = r"Of these {N} ratings, {N}% are Buy ratings, {N}% are Hold ratings, and {N}% are Sell ratings";
    SECTOR_PE = r"({NAME}) \({N}\)";
    SECTOR_PE_AT = r"the ({NAME}) sector has the (?:highest|lowest) {FPE} at {N}";
    TABLE_ROW = r"(S&P 500|{NAME}) {N} {N} {N} {N}";
}

static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid pattern"));
static BROKEN_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\w)- (\w)").expect("valid pattern"));

#[derive(Default)]
struct Candidates(BTreeMap<(EstimateMetric, EstimateScope, Option<EstimatePeriod>), Vec<Found>>);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Found {
    value: Option<Decimal>,
    as_of: Option<NaiveDate>,
}

impl Candidates {
    fn add(
        &mut self,
        metric: EstimateMetric,
        scope: EstimateScope,
        period: Option<EstimatePeriod>,
        value: Option<Decimal>,
        as_of: Option<NaiveDate>,
    ) {
        if value.is_some() || as_of.is_some() {
            self.0
                .entry((metric, scope, period))
                .or_default()
                .push(Found { value, as_of });
        }
    }

    fn index(&mut self, metric: EstimateMetric, value: Option<Decimal>) {
        self.add(metric, EstimateScope::Sp500, None, value, None);
    }

    fn agreed(self) -> Vec<EstimateFact> {
        self.0
            .into_iter()
            .filter_map(|((metric, scope, period), found)| {
                let mut values = found.iter().filter_map(|other| other.value);
                let value = values.next()?;
                if values.any(|other| other != value) {
                    return None;
                }
                let mut dates = found.iter().filter_map(|other| other.as_of);
                let as_of = dates
                    .next()
                    .filter(|first| dates.all(|date| date == *first));
                Some(EstimateFact {
                    metric,
                    scope,
                    period,
                    as_of,
                    value: value.normalize(),
                })
            })
            .collect()
    }
}

pub fn parse(report: &EarningsInsight) -> Vec<EstimateFact> {
    let text = normalize(&report.text);
    let mut found = Candidates::default();
    let sentences = sentences(&text);
    for (index, sentence) in sentences.iter().enumerate() {
        let previous = index
            .checked_sub(1)
            .map_or("", |previous| sentences[previous]);
        valuation(&mut found, sentence, previous, report.issue_date);
        estimates(&mut found, sentence);
    }
    tables(&mut found, &text);
    found.agreed()
}

fn normalize(text: &str) -> String {
    let text = text
        .replace(['\u{2019}', '\u{2018}'], "'")
        .replace(['\u{201c}', '\u{201d}'], "\"")
        .replace('\u{a0}', " ")
        .replace("S&P500", "S&P 500");
    let text = SPACES.replace_all(&text, " ");
    BROKEN_WORD.replace_all(&text, "$1-$2").into_owned()
}

fn sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        let ends = matches!(byte, b'.' | b'!' | b'?')
            && bytes.get(index + 1) == Some(&b' ')
            && bytes
                .get(index + 2)
                .is_some_and(|next| !next.is_ascii_digit() && !next.is_ascii_lowercase());
        if ends {
            sentences.push(text[start..=index].trim());
            start = index + 1;
        }
    }
    sentences.push(text[start..].trim());
    sentences
}

fn number(text: &str) -> Option<Decimal> {
    text.replace(',', "").parse().ok()
}

fn group(captures: &Captures<'_>, index: usize) -> Option<Decimal> {
    number(captures.get(index)?.as_str())
}

fn quarter(captures: &Captures<'_>, quarter: usize, year: usize) -> Option<EstimatePeriod> {
    Some(EstimatePeriod::Quarter(
        captures.get(year)?.as_str().parse().ok()?,
        captures.get(quarter)?.as_str().parse().ok()?,
    ))
}

fn quarter_before(sentence: &str, captures: &Captures<'_>) -> Option<EstimatePeriod> {
    let before = &sentence[..captures.get(0)?.start()];
    SENTENCE_QUARTER
        .captures_iter(before)
        .last()
        .and_then(|captures| quarter(&captures, 1, 2))
}

fn about_the_index(sentence: &str, before: usize) -> bool {
    let lead = sentence[..before].to_ascii_lowercase();
    let lead = lead.trim_end();
    !(lead.ends_with("average")
        || lead.ends_with("'s")
        || lead.ends_with("s'")
        || lead.ends_with("sector")
        || lead.ends_with("prior"))
}

fn valuation(found: &mut Candidates, sentence: &str, previous: &str, issued: NaiveDate) {
    let lower = sentence.to_ascii_lowercase();
    let trailing = lower.contains("trailing 12-month p/e");
    let sector = lower.contains("sector");
    for captures in STATED_PE.captures_iter(sentence) {
        let whole = captures.get(0).expect("the match");
        if !trailing && about_the_index(sentence, whole.start()) {
            found.index(EstimateMetric::ForwardPe, group(&captures, 1));
        }
    }
    for regex in [&*CURRENT_PE, &*INDEX_PE] {
        for captures in regex.captures_iter(sentence) {
            found.index(EstimateMetric::ForwardPe, group(&captures, 1));
        }
    }
    let mut dated = None;
    for captures in DATED_PE
        .captures_iter(sentence)
        .chain(DATED_PE.captures_iter(previous))
    {
        let as_of = month_day(&captures[1], &captures[2], issued);
        dated = dated.or(as_of);
        found.add(
            EstimateMetric::ForwardPe,
            EstimateScope::Sp500,
            None,
            group(&captures, 3),
            as_of,
        );
    }
    for captures in BASIS.captures_iter(sentence) {
        let as_of = match captures.get(1) {
            Some(day) => day_before(day.as_str(), issued),
            None => dated,
        };
        for (metric, index) in [
            (EstimateMetric::IndexPrice, 2),
            (EstimateMetric::ForwardEps, 3),
        ] {
            found.add(
                metric,
                EstimateScope::Sp500,
                None,
                group(&captures, index),
                as_of,
            );
        }
        found.add(
            EstimateMetric::ForwardPe,
            EstimateScope::Sp500,
            None,
            None,
            as_of,
        );
    }
    if trailing {
        for captures in TRAILING_PE.captures_iter(sentence) {
            found.index(EstimateMetric::TrailingPe, group(&captures, 1));
        }
    }
    let mentions_pe = lower.contains("p/e");
    if mentions_pe && !sector {
        let (five, ten) = if trailing {
            (
                EstimateMetric::TrailingPe5yAvg,
                EstimateMetric::TrailingPe10yAvg,
            )
        } else {
            (
                EstimateMetric::ForwardPe5yAvg,
                EstimateMetric::ForwardPe10yAvg,
            )
        };
        for captures in AVERAGE
            .captures_iter(sentence)
            .chain(PAST_AVERAGE.captures_iter(sentence))
            .chain(BRACKETED_AVERAGE.captures_iter(sentence))
        {
            let metric = match captures[1].to_ascii_lowercase().as_str() {
                "5" | "five" => five,
                _ => ten,
            };
            found.index(metric, group(&captures, 2));
        }
    }
    if mentions_pe && sector && !trailing && (lower.contains("highest") || lower.contains("lowest"))
    {
        for captures in SECTOR_PE
            .captures_iter(sentence)
            .chain(SECTOR_PE_AT.captures_iter(sentence))
        {
            if let Some(scope) = sector_scope(&captures[1]) {
                found.add(
                    EstimateMetric::ForwardPe,
                    scope,
                    None,
                    group(&captures, 2),
                    None,
                );
            }
        }
    }
}

fn estimates(found: &mut Candidates, sentence: &str) {
    for captures in CURRENT_GROWTH.captures_iter(sentence) {
        let period = quarter(&captures, 1, 2).or_else(|| quarter(&captures, 5, 6));
        let metric = if captures[3].eq_ignore_ascii_case("revenue") {
            EstimateMetric::RevenueGrowth
        } else {
            EstimateMetric::EpsGrowth
        };
        let growth = group(&captures, 7).map(|value| {
            if captures[4].eq_ignore_ascii_case("decline") {
                -value.abs()
            } else {
                value
            }
        });
        if period.is_some() {
            found.add(metric, EstimateScope::Sp500, period, growth, None);
        }
    }
    for captures in DECLINES.captures_iter(sentence) {
        let Some(period) = quarter_before(sentence, &captures) else {
            continue;
        };
        for (metric, index) in [
            (EstimateMetric::EpsGrowth, 1),
            (EstimateMetric::RevenueGrowth, 2),
        ] {
            let decline = group(&captures, index).map(|value| -value.abs());
            found.add(metric, EstimateScope::Sp500, Some(period), decline, None);
        }
    }
    for captures in PROJECTED_GROWTH.captures_iter(sentence) {
        let period = quarter(&captures, 1, 2).or_else(|| {
            Some(EstimatePeriod::Year(
                captures.get(3)?.as_str().parse().ok()?,
            ))
        });
        for (metric, index) in [
            (EstimateMetric::EpsGrowth, 4),
            (EstimateMetric::RevenueGrowth, 5),
        ] {
            found.add(
                metric,
                EstimateScope::Sp500,
                period,
                group(&captures, index),
                None,
            );
        }
    }
    for (regex, metric) in [
        (&*EPS_BEATS, EstimateMetric::EpsBeatPct),
        (&*REVENUE_BEATS, EstimateMetric::RevenueBeatPct),
    ] {
        for captures in regex.captures_iter(sentence) {
            let period = quarter_before(sentence, &captures);
            if period.is_some() {
                found.add(
                    metric,
                    EstimateScope::Sp500,
                    period,
                    group(&captures, 1),
                    None,
                );
            }
        }
    }
    for captures in GUIDANCE.captures_iter(sentence) {
        let period = quarter(&captures, 1, 2);
        for (metric, index) in [
            (EstimateMetric::NegativeGuidance, 3),
            (EstimateMetric::PositiveGuidance, 4),
        ] {
            found.add(
                metric,
                EstimateScope::Sp500,
                period,
                group(&captures, index),
                None,
            );
        }
    }
    for captures in TARGET.captures_iter(sentence) {
        found.index(EstimateMetric::TargetPrice, group(&captures, 1));
        let upside = group(&captures, 2).map(|upside| match &captures[3] {
            "below" | "Below" => -upside,
            _ => upside,
        });
        found.index(EstimateMetric::TargetUpsidePct, upside);
        found.index(EstimateMetric::IndexPrice, group(&captures, 4));
    }
    for captures in RATINGS.captures_iter(sentence) {
        for (metric, index) in [
            (EstimateMetric::Ratings, 1),
            (EstimateMetric::BuyPct, 2),
            (EstimateMetric::HoldPct, 3),
            (EstimateMetric::SellPct, 4),
        ] {
            found.index(metric, group(&captures, index));
        }
    }
}

fn tables(found: &mut Candidates, text: &str) {
    for (header, columns) in [
        (
            "Sector Mkt Cap Price EPS P/E ",
            [
                None,
                Some(EstimateMetric::IndexPrice),
                Some(EstimateMetric::ForwardEps),
                Some(EstimateMetric::ForwardPe),
            ],
        ),
        (
            "Sector Today 1 Year Avg. 5 Year Avg. 10 Year Avg. ",
            [
                Some(EstimateMetric::ForwardPe),
                None,
                Some(EstimateMetric::ForwardPe5yAvg),
                Some(EstimateMetric::ForwardPe10yAvg),
            ],
        ),
    ] {
        let Some(start) = text.find(header) else {
            continue;
        };
        let mut rest = &text[start + header.len()..];
        while let Some(captures) = TABLE_ROW.captures(rest) {
            let whole = captures.get(0).expect("the match");
            if whole.start() != 0 {
                break;
            }
            let Some(scope) = table_scope(&captures[1]) else {
                break;
            };
            for (metric, index) in columns.iter().zip(2..) {
                let Some(metric) = metric else {
                    continue;
                };
                let is_index = scope == EstimateScope::Sp500;
                if is_index || *metric == EstimateMetric::ForwardPe {
                    found.add(*metric, scope, None, group(&captures, index), None);
                }
            }
            rest = rest[whole.end()..].trim_start();
        }
    }
}

fn table_scope(name: &str) -> Option<EstimateScope> {
    match name {
        "S&P 500" => Some(EstimateScope::Sp500),
        other => sector_scope(other),
    }
}

fn sector_scope(name: &str) -> Option<EstimateScope> {
    SECTORS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, scope)| *scope)
}

fn month_day(month: &str, day: &str, issued: NaiveDate) -> Option<NaiveDate> {
    let month = chrono::Month::try_from(
        u8::try_from(
            [
                "january",
                "february",
                "march",
                "april",
                "may",
                "june",
                "july",
                "august",
                "september",
                "october",
                "november",
                "december",
            ]
            .iter()
            .position(|name| name.eq_ignore_ascii_case(month))?
                + 1,
        )
        .ok()?,
    )
    .ok()?;
    let day: u32 = day.parse().ok()?;
    [issued.year(), issued.year() - 1]
        .into_iter()
        .filter_map(|year| NaiveDate::from_ymd_opt(year, month.number_from_month(), day))
        .find(|date| *date <= issued)
}

fn day_before(name: &str, issued: NaiveDate) -> Option<NaiveDate> {
    let weekday = match name.to_ascii_lowercase().as_str() {
        "yesterday" => return issued.checked_sub_signed(TimeDelta::days(1)),
        "monday" => Weekday::Mon,
        "tuesday" => Weekday::Tue,
        "wednesday" => Weekday::Wed,
        "thursday" => Weekday::Thu,
        "friday" => Weekday::Fri,
        _ => return None,
    };
    (1..=7)
        .filter_map(|back| issued.checked_sub_signed(TimeDelta::days(back)))
        .find(|date| date.weekday() == weekday)
}
