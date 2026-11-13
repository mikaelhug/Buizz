use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use buizz_core::market::{Calendar, FundamentalFact, FundamentalMetric, PeriodType, Symbol};
use buizz_core::visibility::fundamental_visible_at;
use chrono::{NaiveDate, TimeDelta};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Deserializer, de};

use super::concepts::{
    COVER_PAGE_SHARES, DEI, IFRS, IFRS_PRIMARY, US_GAAP, US_GAAP_PRIMARY, WEIGHTED_AVERAGE_SHARES,
};
use super::{ACCEPTED_FORMS, UnitKind};
use crate::http::{SourceError, json_decimal};

const SHARES_GAP_DAYS: i64 = 90;
const SHARE_SCALE_REFERENCE_DAYS: i64 = 400;
const SCALE_SHIFTS: [i32; 5] = [-2, -1, 0, 1, 2];
const SAME_SCALE_ORDERS: f64 = 0.5;

#[derive(Deserialize)]
struct CompanyFacts {
    #[serde(default)]
    facts: BTreeMap<String, Mapped>,
}

struct Mapped(BTreeMap<String, Concept>);

fn mapped(concept: &str) -> bool {
    concept == COVER_PAGE_SHARES
        || WEIGHTED_AVERAGE_SHARES.contains(&concept)
        || US_GAAP_PRIMARY
            .iter()
            .chain(IFRS_PRIMARY)
            .any(|&(name, _, _)| name == concept)
}

impl<'de> Deserialize<'de> for Mapped {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Mapped;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of XBRL concepts")
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Mapped, A::Error> {
                let mut concepts = BTreeMap::new();
                while let Some(name) = map.next_key::<String>()? {
                    if mapped(&name) {
                        concepts.insert(name, map.next_value::<Concept>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(Mapped(concepts))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
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

type Claim = (FundamentalMetric, NaiveDate, PeriodType, String);

struct Collector<'a> {
    symbol: &'a Symbol,
    cik: u32,
    calendar: &'a Calendar,
    share_counts: ShareCounts<'a>,
    facts: Vec<FundamentalFact>,
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
    cik: u32,
    payload: &[u8],
    calendar: &Calendar,
) -> Result<Vec<FundamentalFact>, SourceError> {
    let company: CompanyFacts =
        serde_json::from_slice(payload).map_err(|error| SourceError::Decode(error.to_string()))?;
    let lookup = |taxonomy: &str, concept: &str| company.facts.get(taxonomy)?.0.get(concept);
    let fallbacks: Vec<(&str, &str)> = WEIGHTED_AVERAGE_SHARES
        .iter()
        .map(|concept| (US_GAAP, *concept))
        .chain([(DEI, COVER_PAGE_SHARES)])
        .collect();
    let share_concepts = [(US_GAAP, US_GAAP_PRIMARY), (IFRS, IFRS_PRIMARY)]
        .into_iter()
        .flat_map(|(taxonomy, table)| {
            table
                .iter()
                .filter(|(_, _, kind)| *kind == UnitKind::Shares)
                .map(move |(concept, _, _)| (taxonomy, *concept))
        })
        .chain(fallbacks.iter().copied());
    let mut collector = Collector {
        symbol,
        cik,
        calendar,
        share_counts: ShareCounts::new(share_concepts.filter_map(|(taxonomy, concept)| {
            Some((taxonomy, concept, lookup(taxonomy, concept)?))
        })),
        facts: Vec::new(),
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

    for (taxonomy, concept) in fallbacks {
        if let Some(node) = lookup(taxonomy, concept) {
            let covered = collector.covered_share_periods(taxonomy == DEI);
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

    Ok(drop_repeated_values(collector.facts))
}

fn drop_repeated_values(mut facts: Vec<FundamentalFact>) -> Vec<FundamentalFact> {
    let period = |fact: &FundamentalFact| {
        (
            fact.metric,
            fact.period_end,
            fact.period_type,
            fact.period_start,
        )
    };
    facts.sort_by(|a, b| {
        (period(a), a.filed, &a.accession).cmp(&(period(b), b.filed, &b.accession))
    });
    let mut kept: Vec<FundamentalFact> = Vec::with_capacity(facts.len());
    for fact in facts {
        let repeated = kept
            .last()
            .is_some_and(|last| period(last) == period(&fact) && last.value == fact.value);
        if !repeated {
            kept.push(fact);
        }
    }
    kept
}

impl Collector<'_> {
    fn covered_share_periods(&self, instants: bool) -> Vec<NaiveDate> {
        self.facts
            .iter()
            .filter(|fact| fact.metric == Some(FundamentalMetric::SharesOutstanding))
            .filter(|fact| !instants || fact.period_start.is_none())
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
            if source.kind == UnitKind::Shares {
                value = self.share_counts.whole(ReportedCount {
                    period: (observation.start, observation.end),
                    concept: (source.taxonomy, source.concept),
                    value,
                });
            }
            if let Some(covered) = clear_of
                && !covered
                    .iter()
                    .all(|period| (observation.end - *period).num_days().abs() > SHARES_GAP_DAYS)
            {
                continue;
            }
            let claim = (
                source.metric,
                observation.end,
                period_type,
                observation.accn.clone(),
            );
            let owner = (source.taxonomy, source.concept);
            if *self.claimed.entry(claim).or_insert(owner) != owner {
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

type ConceptKey<'a> = (&'a str, &'a str);
type Period = (Option<NaiveDate>, NaiveDate);

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct ReportedCount<'a> {
    period: Period,
    concept: ConceptKey<'a>,
    value: Decimal,
}

struct ShareCounts<'a>(Vec<(ReportedCount<'a>, f64)>);

impl<'a> ShareCounts<'a> {
    fn new(concepts: impl Iterator<Item = (&'a str, &'a str, &'a Concept)>) -> Self {
        let distinct: BTreeSet<ReportedCount<'a>> = concepts
            .filter_map(|(taxonomy, concept, node)| {
                Some(
                    node.units
                        .get("shares")?
                        .iter()
                        .filter_map(move |observation| {
                            Some(ReportedCount {
                                period: (observation.start, observation.end),
                                concept: (taxonomy, concept),
                                value: json_decimal(&observation.val)?,
                            })
                        }),
                )
            })
            .flatten()
            .collect();
        let mut counts: Vec<_> = distinct
            .into_iter()
            .filter_map(|count| {
                let magnitude = magnitude(count.value)?;
                Some((count, magnitude))
            })
            .collect();
        counts.sort_by_key(|(count, _)| count.period.1);
        Self(counts)
    }

    fn whole(&self, reported: ReportedCount<'_>) -> Decimal {
        let Some(magnitude) = magnitude(reported.value) else {
            return reported.value;
        };
        let end = reported.period.1;
        let window = TimeDelta::days(SHARE_SCALE_REFERENCE_DAYS);
        let from = self
            .0
            .partition_point(|(count, _)| count.period.1 < end - window);
        let to = self
            .0
            .partition_point(|(count, _)| count.period.1 <= end + window);
        let others: Vec<&(ReportedCount, f64)> = self.0[from..to]
            .iter()
            .filter(|(count, _)| {
                (count.period, count.concept, count.value)
                    != (reported.period, reported.concept, reported.value)
            })
            .collect();
        let concepts: BTreeSet<ConceptKey> =
            others.iter().map(|(count, _)| count.concept).collect();
        let mut scores = [0.0; SCALE_SHIFTS.len()];
        for concept in concepts {
            let support = SCALE_SHIFTS.map(|shift| {
                others
                    .iter()
                    .filter(|(count, other)| {
                        count.concept == concept
                            && (other - magnitude - f64::from(3 * shift)).abs() <= SAME_SCALE_ORDERS
                    })
                    .count() as f64
            });
            let total: f64 = support.iter().sum();
            if total > 0.0 {
                for (score, agreeing) in scores.iter_mut().zip(support) {
                    *score += agreeing / total;
                }
            }
        }
        let best = scores.iter().copied().fold(0.0, f64::max);
        let leaders: Vec<i32> = SCALE_SHIFTS
            .into_iter()
            .zip(scores)
            .filter(|(_, score)| *score == best)
            .map(|(shift, _)| shift)
            .collect();
        let [shift] = leaders[..] else {
            return reported.value;
        };
        if shift == 0 {
            return reported.value;
        }
        let factor = Decimal::from(1000_i64.pow(shift.unsigned_abs()));
        let whole = if shift < 0 {
            reported.value / factor
        } else {
            reported.value * factor
        };
        whole.normalize()
    }
}

fn magnitude(value: Decimal) -> Option<f64> {
    (value > Decimal::ZERO).then(|| value.to_f64().map(f64::log10))?
}
