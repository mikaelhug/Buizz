use std::collections::{BTreeMap, HashMap, HashSet};

use buizz_core::market::{Calendar, FundamentalFact, FundamentalMetric, PeriodType, Symbol};
use buizz_core::visibility::fundamental_visible_at;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;

use super::concepts::{
    COVER_PAGE_SHARES, DEI, IFRS, IFRS_PRIMARY, US_GAAP, US_GAAP_PRIMARY, WEIGHTED_AVERAGE_SHARES,
};
use super::{ACCEPTED_FORMS, UnitKind};
use crate::http::{SourceError, json_decimal};

const SHARES_GAP_DAYS: i64 = 90;
const SHARE_SCALE_MIN: i64 = 1_000;
const SHARE_SCALE_REFERENCE_DAYS: i64 = 400;

#[derive(Deserialize)]
struct CompanyFacts {
    cik: u32,
    facts: BTreeMap<String, BTreeMap<String, Concept>>,
}

#[derive(Deserialize)]
struct Concept {
    units: BTreeMap<String, Vec<Observation>>,
}

#[derive(Deserialize)]
struct Observation {
    start: Option<NaiveDate>,
    end: NaiveDate,
    val: serde_json::Number,
    fy: Option<i32>,
    fp: Option<String>,
    form: String,
    filed: NaiveDate,
    accn: String,
    frame: Option<String>,
}

type Claim = (FundamentalMetric, NaiveDate, PeriodType);

struct Collector<'a> {
    symbol: &'a Symbol,
    cik: u32,
    calendar: &'a Calendar,
    cover_counts: Vec<(NaiveDate, Decimal)>,
    facts: Vec<FundamentalFact>,
    seen: HashSet<(FundamentalMetric, NaiveDate, PeriodType, Decimal)>,
    claimed: HashMap<Claim, (&'static str, &'static str)>,
}

struct Source<'a> {
    taxonomy: &'static str,
    concept: &'static str,
    node: &'a Concept,
    metric: FundamentalMetric,
    kind: UnitKind,
}

pub fn normalize_company_facts(
    symbol: &Symbol,
    payload: &[u8],
    calendar: &Calendar,
) -> Result<Vec<FundamentalFact>, SourceError> {
    let company: CompanyFacts =
        serde_json::from_slice(payload).map_err(|error| SourceError::Decode(error.to_string()))?;
    let lookup = |taxonomy: &str, concept: &str| company.facts.get(taxonomy)?.get(concept);
    let mut collector = Collector {
        symbol,
        cik: company.cik,
        calendar,
        cover_counts: cover_counts(lookup(DEI, COVER_PAGE_SHARES)),
        facts: Vec::new(),
        seen: HashSet::new(),
        claimed: HashMap::new(),
    };

    for (taxonomy, table) in [(US_GAAP, US_GAAP_PRIMARY), (IFRS, IFRS_PRIMARY)] {
        for &(concept, metric, kind) in table {
            if let Some(node) = lookup(taxonomy, concept) {
                collector.emit(
                    &Source {
                        taxonomy,
                        concept,
                        node,
                        metric,
                        kind,
                    },
                    None,
                );
            }
        }
    }

    let fallbacks = WEIGHTED_AVERAGE_SHARES
        .iter()
        .map(|concept| (US_GAAP, *concept))
        .chain([(DEI, COVER_PAGE_SHARES)]);
    for (taxonomy, concept) in fallbacks {
        if let Some(node) = lookup(taxonomy, concept) {
            let covered = collector.covered_share_periods();
            collector.emit(
                &Source {
                    taxonomy,
                    concept,
                    node,
                    metric: FundamentalMetric::SharesOutstanding,
                    kind: UnitKind::Shares,
                },
                Some(&covered),
            );
        }
    }

    let mut facts = collector.facts;
    facts.sort_by_key(|fact| (fact.metric, fact.period_end, fact.period_type));
    Ok(facts)
}

impl Collector<'_> {
    fn covered_share_periods(&self) -> Vec<NaiveDate> {
        self.facts
            .iter()
            .filter(|fact| fact.metric == Some(FundamentalMetric::SharesOutstanding))
            .map(|fact| fact.period_end)
            .collect()
    }

    fn emit(&mut self, source: &Source, clear_of: Option<&[NaiveDate]>) {
        let Some(unit) = pick_unit(&source.node.units, source.kind) else {
            return;
        };
        let mut observations: Vec<&Observation> = source.node.units[unit].iter().collect();
        observations.sort_by(|a, b| (a.filed, &a.accn).cmp(&(b.filed, &b.accn)));
        for observation in observations {
            if !ACCEPTED_FORMS.contains(&observation.form.as_str()) {
                continue;
            }
            let Some(period_type) = period_type(observation) else {
                continue;
            };
            let Some(mut value) = json_decimal(&observation.val) else {
                continue;
            };
            if source.kind == UnitKind::Shares && source.taxonomy != DEI {
                value = to_whole_shares(value, observation.end, &self.cover_counts);
            }
            if let Some(covered) = clear_of
                && !covered
                    .iter()
                    .all(|period| (observation.end - *period).num_days().abs() > SHARES_GAP_DAYS)
            {
                continue;
            }
            let claim = (source.metric, observation.end, period_type);
            let owner = (source.taxonomy, source.concept);
            if self.claimed.get(&claim).is_some_and(|held| *held != owner) {
                continue;
            }
            self.claimed.insert(claim, owner);
            if !self
                .seen
                .insert((source.metric, observation.end, period_type, value))
            {
                continue;
            }
            let Some(available_at) = fundamental_visible_at(observation.filed, self.calendar)
            else {
                continue;
            };
            self.facts.push(FundamentalFact {
                symbol: self.symbol.clone(),
                cik: self.cik,
                taxonomy: source.taxonomy.to_owned(),
                concept: source.concept.to_owned(),
                unit: unit.to_owned(),
                metric: Some(source.metric),
                period_type: Some(period_type),
                period_start: observation.start,
                period_end: observation.end,
                value,
                fiscal_year: observation.fy,
                fiscal_period: observation.fp.clone(),
                form: observation.form.clone(),
                accession: observation.accn.clone(),
                filed: observation.filed,
                frame: observation.frame.clone(),
                available_at,
            });
        }
    }
}

fn pick_unit(units: &BTreeMap<String, Vec<Observation>>, kind: UnitKind) -> Option<&str> {
    let is_currency =
        |key: &str| key.len() == 3 && key.chars().all(|letter| letter.is_ascii_uppercase());
    let keys = || units.keys().map(String::as_str);
    match kind {
        UnitKind::Shares => keys().find(|key| *key == "shares"),
        UnitKind::Currency => keys()
            .find(|key| *key == "USD")
            .or_else(|| keys().find(|key| is_currency(key))),
        UnitKind::CurrencyPerShare => keys()
            .find(|key| *key == "USD/shares")
            .or_else(|| keys().find(|key| key.strip_suffix("/shares").is_some_and(is_currency))),
    }
}

fn period_type(observation: &Observation) -> Option<PeriodType> {
    let fiscal_period = observation.fp.as_deref();
    match observation.start {
        Some(start) => match (observation.end - start).num_days() {
            80..=100 => Some(PeriodType::Quarter),
            350..=380 if matches!(fiscal_period, None | Some("FY")) => Some(PeriodType::Annual),
            _ => None,
        },
        None => match fiscal_period {
            Some("FY") => Some(PeriodType::Annual),
            Some("Q1" | "Q2" | "Q3" | "Q4") => Some(PeriodType::Quarter),
            _ => None,
        },
    }
}

fn cover_counts(node: Option<&Concept>) -> Vec<(NaiveDate, Decimal)> {
    let mut counts: Vec<_> = node
        .and_then(|concept| concept.units.get("shares"))
        .into_iter()
        .flatten()
        .filter_map(|observation| Some((observation.end, json_decimal(&observation.val)?)))
        .filter(|(_, value)| *value > Decimal::ZERO)
        .collect();
    counts.sort();
    counts
}

fn to_whole_shares(
    value: Decimal,
    period_end: NaiveDate,
    cover: &[(NaiveDate, Decimal)],
) -> Decimal {
    if value <= Decimal::ZERO {
        return value;
    }
    let Some((cover_end, cover_count)) = cover
        .iter()
        .min_by_key(|(end, _)| (*end - period_end).num_days().abs())
    else {
        return value;
    };
    if (*cover_end - period_end).num_days().abs() > SHARE_SCALE_REFERENCE_DAYS {
        return value;
    }
    let ratio = cover_count / value;
    if ratio < Decimal::from(SHARE_SCALE_MIN) {
        return value;
    }
    let mut lower = Decimal::ONE;
    while ratio >= lower * Decimal::TEN {
        lower *= Decimal::TEN;
    }
    let upper = lower * Decimal::TEN;
    let factor = if ratio / lower <= upper / ratio {
        lower
    } else {
        upper
    };
    if (ratio - factor).abs() / factor > Decimal::new(1, 1) {
        return value;
    }
    (value * factor).normalize()
}
