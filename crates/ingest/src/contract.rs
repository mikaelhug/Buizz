use std::collections::{BTreeMap, BTreeSet};

use buizz_core::api::{
    ContractState, CoverageContract, JobView, RecipeContract, RecipeVersionView, ScopeKind,
    ScopeUnit, SpineEvent,
};
use buizz_core::market::{
    ALPACA_HISTORY_START, AssetClass, Calendar, Coverage, DataSource, Dataset, MacroSeries,
    RecipeState, Symbol,
};
use buizz_core::ops::{FetchStatus, JobOutcome};
use buizz_core::time::Timestamp;
use buizz_core::visibility::short_interest_published;
use buizz_market::AccessionProgress;
use buizz_sources::alpaca::NEWS_HISTORY_START;
use buizz_sources::edgar::SPLITTER_VERSION;
use buizz_sources::finra::{FIRST_SETTLEMENT, FIRST_SHORT_VOLUME};
use buizz_state::{ContractHistory, SymbolFetches};
use chrono::{NaiveDate, TimeDelta};

use crate::delistings::{next_quarter, quarter_span};
use crate::fetch::read;
use crate::finra::settlement_dates;
use crate::insiders::insider_quarters_due;
use crate::macros::series_key;
use crate::recipe::RECIPES;
use crate::schedule::JOBS;
use crate::{Ingest, IngestError};

const PACE_WINDOW: TimeDelta = TimeDelta::days(30);
const STALL_AFTER: TimeDelta = TimeDelta::days(3);
const MIN_PACE_SPAN: TimeDelta = TimeDelta::hours(6);
const SECONDS_PER_DAY: f64 = 86_400.0;
const RECENT_LISTINGS: u32 = 20;

#[derive(Clone, Copy)]
enum Item {
    Done,
    Failed,
    Pending,
}

impl Item {
    fn backfilled(fetches: Option<&SymbolFetches>) -> Self {
        match fetches {
            Some(SymbolFetches {
                latest: FetchStatus::Rejected,
                ..
            }) => Self::Failed,
            Some(SymbolFetches { read: true, .. }) => Self::Done,
            Some(SymbolFetches {
                latest: FetchStatus::Failed,
                ..
            }) => Self::Failed,
            _ => Self::Pending,
        }
    }

    fn current(fetches: Option<&SymbolFetches>) -> Self {
        match fetches.map(|fetches| fetches.latest) {
            Some(FetchStatus::Ok | FetchStatus::Empty) => Self::Done,
            Some(FetchStatus::Failed | FetchStatus::Rejected) => Self::Failed,
            _ => Self::Pending,
        }
    }
}

struct Scope {
    dataset: Dataset,
    source: DataSource,
    kind: ScopeKind,
    unit: ScopeUnit,
    scope: String,
    from: NaiveDate,
    to: NaiveDate,
}

impl Scope {
    fn items(self, items: impl IntoIterator<Item = Item>) -> Measure {
        let (mut expected, mut done, mut failed) = (0, 0, 0);
        for item in items {
            expected += 1;
            match item {
                Item::Done => done += 1,
                Item::Failed => failed += 1,
                Item::Pending => {}
            }
        }
        self.counted(expected, done, failed)
    }

    fn counted(self, expected: u64, done: u64, failed: u64) -> Measure {
        let current = matches!(self.unit, ScopeUnit::Series | ScopeUnit::Lists);
        let complete = done + failed >= expected && (failed == 0 || !current);
        let to = self.to;
        Measure {
            scope: self,
            expected,
            done,
            failed,
            through: complete.then_some(to),
            recipe: None,
        }
    }

    fn accessions(self, progress: AccessionProgress) -> Measure {
        self.counted(progress.expected, progress.done, progress.failed)
    }

    fn through(self, calendar: &Calendar, through: Option<NaiveDate>) -> Measure {
        let count = |from: NaiveDate, to: NaiveDate| -> u64 {
            if from > to {
                return 0;
            }
            match self.unit {
                ScopeUnit::Days => u64::try_from((to - from).num_days() + 1).unwrap_or(0),
                _ => calendar.between(from, to).len() as u64,
            }
        };
        let expected = count(self.from, self.to);
        let done = through.map_or(0, |through| count(self.from, through.min(self.to)));
        let reached = through.map(|through| through.min(self.to));
        Measure {
            expected,
            done,
            failed: 0,
            through: reached,
            scope: self,
            recipe: None,
        }
    }
}

fn feeding(jobs: &[JobView], (dataset, source): (Dataset, DataSource)) -> Vec<&JobView> {
    JOBS.iter()
        .zip(jobs)
        .filter(|(spec, _)| spec.source == Some(source) && spec.datasets.contains(&dataset))
        .map(|(_, view)| view)
        .collect()
}

struct Measure {
    scope: Scope,
    expected: u64,
    done: u64,
    failed: u64,
    through: Option<NaiveDate>,
    recipe: Option<RecipeContract>,
}

impl Ingest {
    pub async fn contracts(&self) -> Result<Vec<CoverageContract>, IngestError> {
        let cached = self.contracts.borrow().clone();
        if cached.is_empty() {
            self.refresh_contracts(Timestamp::now()).await
        } else {
            Ok(cached)
        }
    }

    pub(crate) async fn refresh_contracts(
        &self,
        now: Timestamp,
    ) -> Result<Vec<CoverageContract>, IngestError> {
        let jobs = self.job_views(now).await?;
        let measures = self.measure(now, &jobs).await?;
        let histories = self
            .state
            .track_contracts(
                now,
                PACE_WINDOW,
                measures
                    .iter()
                    .map(|measure| (measure.scope.dataset, measure.scope.source, measure.done))
                    .collect(),
            )
            .await?;
        let contracts: Vec<CoverageContract> = measures
            .into_iter()
            .map(|measure| {
                let key = (measure.scope.dataset, measure.scope.source);
                self.settle(measure, histories.get(&key), &feeding(&jobs, key), now)
            })
            .collect();
        self.contracts.send_replace(contracts.clone());
        let _ = self.events.send(SpineEvent::Contracts {
            contracts: contracts.clone(),
        });
        Ok(contracts)
    }

    fn settle(
        &self,
        measure: Measure,
        history: Option<&ContractHistory>,
        jobs: &[&JobView],
        now: Timestamp,
    ) -> CoverageContract {
        let Measure {
            scope,
            expected,
            done,
            failed,
            through,
            recipe,
        } = measure;
        let pending = expected.saturating_sub(done + failed);
        let per_day = history.and_then(|history| {
            let span = now - history.base_at;
            let gained = done
                .checked_sub(history.base_done)
                .filter(|gained| *gained > 0)?;
            (span >= MIN_PACE_SPAN)
                .then(|| gained as f64 / (span.num_seconds() as f64 / SECONDS_PER_DAY))
        });
        let failing = jobs
            .iter()
            .find(|job| job.stale || job.last_outcome == Some(JobOutcome::Failed));
        let waiting = jobs.iter().any(|job| {
            matches!(
                job.last_outcome,
                Some(JobOutcome::Waiting | JobOutcome::Backoff)
            )
        });
        let still_since = history
            .map(|history| history.moved_at)
            .filter(|moved| *moved + STALL_AFTER < now);
        let stalled = if pending == 0 && failed == 0 {
            None
        } else if let Some(job) = failing {
            Some(format!(
                "{} {}: {}",
                job.name.replace('_', " "),
                if job.stale { "is late" } else { "failed" },
                job.last_message.as_deref().unwrap_or("no message")
            ))
        } else if pending == 0 && through.is_none() {
            Some(format!("{failed} failed on their last read"))
        } else if pending == 0 {
            None
        } else if !self.config.enabled {
            Some("ingest is off on this spine".to_owned())
        } else if jobs.is_empty() && scope.kind == ScopeKind::Recorder {
            still_since.map(|since| format!("nothing recorded since {}", since.exchange_date()))
        } else if waiting {
            None
        } else {
            still_since.map(|since| format!("no progress since {}", since.exchange_date()))
        };
        let state = match (&stalled, pending) {
            (Some(_), _) => ContractState::Stalled,
            (None, 0) => ContractState::Complete,
            (None, _) => ContractState::Filling,
        };
        let eta = per_day
            .filter(|_| state == ContractState::Filling)
            .map(|per_day| {
                let days = (pending as f64 / per_day).ceil().min(36_500.0) as i64;
                now.exchange_date() + TimeDelta::days(days)
            });
        CoverageContract {
            dataset: scope.dataset,
            source: scope.source,
            kind: scope.kind,
            unit: scope.unit,
            scope: scope.scope,
            from: scope.from,
            to: scope.to,
            expected,
            done,
            failed,
            pending,
            complete_through: through,
            state,
            per_day,
            eta,
            reason: stalled,
            recipe,
        }
    }

    async fn measure(&self, now: Timestamp, jobs: &[JobView]) -> Result<Vec<Measure>, IngestError> {
        let today = now.exchange_date();
        let start = self.config.history_start;
        let calendar = self.market.calendar(start, today).await?;
        let due = calendar
            .previous_before(today)
            .map_or(today, |session| session.date);
        let yesterday = today - TimeDelta::days(1);
        let coverage = self.market.coverage().await?;
        let series = |dataset: Dataset, source: DataSource| -> Option<&Coverage> {
            coverage
                .iter()
                .find(|row| row.dataset == dataset && row.source == source && row.symbol.is_none())
        };
        let covered = |dataset: Dataset, source: DataSource| -> BTreeMap<&Symbol, &Coverage> {
            coverage
                .iter()
                .filter(|row| row.dataset == dataset && row.source == source)
                .filter_map(|row| Some((row.symbol.as_ref()?, row)))
                .collect()
        };
        let priced = self.priced_symbols().await?;
        let priced_set: BTreeSet<&Symbol> = priced.iter().map(|p| &p.symbol).collect();
        let stocks: Vec<Symbol> = self
            .market
            .instruments()
            .await?
            .into_iter()
            .filter(|instrument| instrument.active && instrument.asset_class == AssetClass::Stock)
            .map(|instrument| instrument.symbol)
            .filter(|symbol| priced_set.contains(symbol))
            .collect();
        let mut measures = Vec::new();

        let bars = covered(Dataset::Bars1d, DataSource::Alpaca);
        let fetched = self
            .state
            .symbol_fetches(DataSource::Alpaca, Dataset::Bars1d)
            .await?;
        measures.push(
            Scope {
                dataset: Dataset::Bars1d,
                source: DataSource::Alpaca,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Symbols,
                scope: "priced stocks and ETFs".into(),
                from: ALPACA_HISTORY_START,
                to: today,
            }
            .items(priced.iter().map(|p| {
                if bars.contains_key(&p.symbol) {
                    Item::Done
                } else {
                    Item::backfilled(fetched.get(&p.symbol))
                }
            })),
        );

        let actions = covered(Dataset::CorporateActions, DataSource::Alpaca);
        let fetched = self
            .state
            .symbol_fetches(DataSource::Alpaca, Dataset::CorporateActions)
            .await?;
        measures.push(
            Scope {
                dataset: Dataset::CorporateActions,
                source: DataSource::Alpaca,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Symbols,
                scope: "priced stocks and ETFs".into(),
                from: start,
                to: today,
            }
            .items(priced.iter().map(|p| {
                if actions.get(&p.symbol).is_some_and(|row| row.first <= start) {
                    Item::Done
                } else {
                    match Item::backfilled(fetched.get(&p.symbol)) {
                        Item::Failed => Item::Failed,
                        _ => Item::Pending,
                    }
                }
            })),
        );

        if let Some(floor) = bars.values().map(|row| row.first).min() {
            let fetched = self
                .state
                .symbol_fetches(DataSource::Tiingo, Dataset::Bars1d)
                .await?;
            let reaches_back = calendar
                .sessions()
                .first()
                .is_some_and(|first| first.date < floor);
            measures.push(
                Scope {
                    dataset: Dataset::Bars1d,
                    source: DataSource::Tiingo,
                    kind: ScopeKind::Indexed,
                    unit: ScopeUnit::Symbols,
                    scope: "priced symbols listed before Alpaca's history starts".into(),
                    from: start,
                    to: floor - TimeDelta::days(1),
                }
                .items(
                    priced
                        .iter()
                        .filter(|p| {
                            reaches_back
                                && bars.get(&p.symbol).is_some_and(|row| row.first == floor)
                        })
                        .map(|p| Item::backfilled(fetched.get(&p.symbol))),
                ),
            );
        }

        let minute = covered(Dataset::Bars1m, DataSource::Alpaca);
        if let (Some(first), Some(last)) = (
            minute.values().map(|row| row.first).min(),
            minute.values().map(|row| row.last).max(),
        ) {
            measures.push(
                Scope {
                    dataset: Dataset::Bars1m,
                    source: DataSource::Alpaca,
                    kind: ScopeKind::OnDemand,
                    unit: ScopeUnit::Symbols,
                    scope: "symbols and sessions runs asked for".into(),
                    from: first,
                    to: last,
                }
                .items(minute.keys().map(|_| Item::Done)),
            );
        }

        for (dataset, source, scope) in [
            (
                Dataset::Instruments,
                DataSource::Alpaca,
                "Alpaca's list of tradable assets",
            ),
            (
                Dataset::Calendar,
                DataSource::Alpaca,
                "Alpaca's session calendar",
            ),
            (
                Dataset::IndexMembership,
                DataSource::Wikipedia,
                "the S&P 500 change log",
            ),
            (
                Dataset::Fomc,
                DataSource::FederalReserve,
                "the FOMC meeting calendar",
            ),
            (
                Dataset::Estimates,
                DataSource::Factset,
                "Earnings Insight and its archive",
            ),
        ] {
            let from = series(dataset, source).map_or(start, |row| row.first);
            let state =
                feeding(jobs, (dataset, source))
                    .into_iter()
                    .fold(Item::Done, |item, view| {
                        let read = !view.stale
                            && view.last_finished.is_some()
                            && view.last_outcome != Some(JobOutcome::Failed);
                        match (item, read, view.last_outcome) {
                            (Item::Failed, _, _) | (_, false, Some(JobOutcome::Failed)) => {
                                Item::Failed
                            }
                            (_, false, _) => Item::Pending,
                            (item, true, _) => item,
                        }
                    });
            measures.push(
                Scope {
                    dataset,
                    source,
                    kind: ScopeKind::Indexed,
                    unit: ScopeUnit::Lists,
                    scope: scope.into(),
                    from,
                    to: today,
                }
                .items([state]),
            );
        }

        let fetched = self
            .state
            .symbol_fetches(DataSource::Edgar, Dataset::CurrentReports)
            .await?;
        for dataset in [
            Dataset::CurrentReports,
            Dataset::EarningsReleases,
            Dataset::Companies,
        ] {
            measures.push(
                Scope {
                    dataset,
                    source: DataSource::Edgar,
                    kind: ScopeKind::Indexed,
                    unit: ScopeUnit::Symbols,
                    scope: "active priced stocks, read from each company's EDGAR filing list"
                        .into(),
                    from: start,
                    to: today,
                }
                .items(
                    stocks
                        .iter()
                        .map(|symbol| Item::backfilled(fetched.get(symbol))),
                ),
            );
        }
        for (dataset, scope) in [
            (
                Dataset::PeriodicReports,
                "active priced stocks, read from each company's 10-K and 10-Q filing list",
            ),
            (
                Dataset::Fundamentals,
                "active priced stocks, read from each company's XBRL facts",
            ),
            (
                Dataset::InsiderTrades,
                "active priced stocks, each company's Form 4 filings dated by its filing list, from SEC's quarterly data sets and single documents after them",
            ),
        ] {
            let fetched = self
                .state
                .symbol_fetches(DataSource::Edgar, dataset)
                .await?;
            measures.push(
                Scope {
                    dataset,
                    source: DataSource::Edgar,
                    kind: ScopeKind::Indexed,
                    unit: ScopeUnit::Symbols,
                    scope: scope.into(),
                    from: start,
                    to: today,
                }
                .items(
                    stocks
                        .iter()
                        .map(|symbol| Item::backfilled(fetched.get(symbol))),
                ),
            );
        }

        let (first, ended) = insider_quarters_due(start, today);
        let read_quarters = self.market.insider_quarters().await?;
        let listed_through = self
            .state
            .recent_fetches(
                DataSource::Edgar,
                Dataset::InsiderDataSets,
                None,
                RECENT_LISTINGS,
            )
            .await?
            .into_iter()
            .find(read)
            .and_then(|fetch| fetch.requested_to)
            .unwrap_or(quarter_span(ended).1);
        let quarters: Vec<NaiveDate> =
            std::iter::successors(Some(first), |quarter| Some(next_quarter(*quarter)))
                .map(|quarter| quarter_span(quarter).0)
                .take_while(|day| *day <= listed_through)
                .collect();
        measures.push(
            Scope {
                dataset: Dataset::InsiderDataSets,
                source: DataSource::Edgar,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Quarters,
                scope:
                    "every quarter of SEC's insider transactions data sets that it has published"
                        .into(),
                from: quarter_span(first).0,
                to: listed_through,
            }
            .items(quarters.iter().map(|quarter| {
                if read_quarters.contains(quarter) {
                    Item::Done
                } else {
                    Item::Pending
                }
            })),
        );

        measures.push(
            Scope {
                dataset: Dataset::Delistings,
                source: DataSource::Edgar,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Filings,
                scope: "every Form 25 and 25-NSE in EDGAR's full and daily indexes since 2016"
                    .into(),
                from: ALPACA_HISTORY_START,
                to: today,
            }
            .accessions(self.market.delisting_progress().await?),
        );
        if let Some(row) = series(Dataset::SecFilings, DataSource::Edgar) {
            measures.push(
                Scope {
                    dataset: Dataset::SecFilings,
                    source: DataSource::Edgar,
                    kind: ScopeKind::Indexed,
                    unit: ScopeUnit::Sessions,
                    scope: "EDGAR's daily index, every business day since the first read".into(),
                    from: row.first,
                    to: due,
                }
                .through(&calendar, Some(row.last)),
            );
        }
        measures.push(
            Scope {
                dataset: Dataset::PressReleases,
                source: DataSource::Edgar,
                kind: ScopeKind::Derived,
                unit: ScopeUnit::Filings,
                scope: "every earnings release 8-K stored".into(),
                from: start,
                to: today,
            }
            .accessions(self.market.press_release_progress().await?),
        );
        if !self.market.text_volumes().volumes().is_empty() {
            measures.push(
                Scope {
                    dataset: Dataset::FilingDocuments,
                    source: DataSource::Edgar,
                    kind: ScopeKind::Derived,
                    unit: ScopeUnit::Filings,
                    scope: "every 8-K, 10-K and 10-Q accession stored".into(),
                    from: start,
                    to: today,
                }
                .accessions(self.market.filing_text_progress().await?),
            );
            measures.push(
                Scope {
                    dataset: Dataset::FilingSections,
                    source: DataSource::Edgar,
                    kind: ScopeKind::Derived,
                    unit: ScopeUnit::Filings,
                    scope: "every stored 10-K, 10-Q and 8-K main document (8-Ks that list items), split by the current splitter"
                        .into(),
                    from: start,
                    to: today,
                }
                .accessions(self.market.split_progress(SPLITTER_VERSION).await?),
            );
            let versions = self.market.recipe_versions().await?;
            for recipe in &RECIPES {
                let Some(version) = versions
                    .iter()
                    .filter(|version| {
                        version.recipe == recipe.name
                            && matches!(
                                version.state,
                                RecipeState::Filling | RecipeState::Complete | RecipeState::Current
                            )
                    })
                    .max_by_key(|version| version.version)
                else {
                    continue;
                };
                let mut measure = Scope {
                    dataset: recipe.dataset,
                    source: DataSource::Edgar,
                    kind: ScopeKind::Derived,
                    unit: ScopeUnit::Sections,
                    scope: format!(
                        "every 8-K Item {} section{}, read by {} version {}",
                        version.scope.items.join(", "),
                        if version.scope.exhibits.is_empty() {
                            String::new()
                        } else {
                            format!(
                                " through its {} exhibits",
                                version.scope.exhibits.join(", ")
                            )
                        },
                        version.recipe,
                        version.version
                    ),
                    from: version.scope.from.max(start),
                    to: today,
                }
                .accessions(self.market.recipe_progress(version.clone()).await?);
                measure.recipe = Some(RecipeContract {
                    recipe: recipe.name.to_owned(),
                    versions: versions
                        .iter()
                        .filter(|known| known.recipe == recipe.name)
                        .map(|known| RecipeVersionView {
                            version: known.version,
                            state: known.state,
                            producer: known.producer.clone(),
                        })
                        .collect(),
                });
                measures.push(measure);
            }
        }

        measures.push(
            Scope {
                dataset: Dataset::Rates,
                source: DataSource::Treasury,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Sessions,
                scope: "every session's yield curve".into(),
                from: series(Dataset::Rates, DataSource::Treasury).map_or(start, |row| row.first),
                to: due,
            }
            .through(
                &calendar,
                series(Dataset::Rates, DataSource::Treasury).map(|row| row.last),
            ),
        );
        for (source, wanted, scope) in [
            (
                DataSource::Fred,
                &MacroSeries::FRED[..],
                "every FRED series with its vintages, current",
            ),
            (
                DataSource::Cboe,
                &MacroSeries::CBOE[..],
                "the VIX and VIX3M closes, current",
            ),
        ] {
            let fetched = self.state.symbol_fetches(source, Dataset::Macro).await?;
            measures.push(
                Scope {
                    dataset: Dataset::Macro,
                    source,
                    kind: ScopeKind::Indexed,
                    unit: ScopeUnit::Series,
                    scope: scope.into(),
                    from: start,
                    to: today,
                }
                .items(
                    wanted
                        .iter()
                        .map(|series| Item::current(fetched.get(&series_key(*series)))),
                ),
            );
        }

        let news_from = NEWS_HISTORY_START.max(start);
        measures.push(
            Scope {
                dataset: Dataset::News,
                source: DataSource::Alpaca,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Days,
                scope: "every day of Alpaca's news history".into(),
                from: news_from,
                to: yesterday,
            }
            .through(
                &calendar,
                series(Dataset::News, DataSource::Alpaca).map(|row| row.last),
            ),
        );
        for (dataset, source, unit, scope) in [
            (
                Dataset::News,
                DataSource::AlpacaStream,
                ScopeUnit::Days,
                "every day since the news stream began recording",
            ),
            (
                Dataset::Borrow,
                DataSource::InteractiveBrokers,
                ScopeUnit::Sessions,
                "every session since borrow recording began",
            ),
            (
                Dataset::OptionChains,
                DataSource::Alpaca,
                ScopeUnit::Sessions,
                "every session since option chain recording began",
            ),
            (
                Dataset::TradingHalts,
                DataSource::Nasdaq,
                ScopeUnit::Days,
                "every day since the Nasdaq halt feed began recording",
            ),
            (
                Dataset::EtfHoldings,
                DataSource::StateStreet,
                ScopeUnit::Sessions,
                "every session since SPDR holdings recording began",
            ),
        ] {
            if let Some(row) = series(dataset, source) {
                measures.push(
                    Scope {
                        dataset,
                        source,
                        kind: ScopeKind::Recorder,
                        unit,
                        scope: scope.into(),
                        from: row.first,
                        to: if unit == ScopeUnit::Days {
                            yesterday
                        } else {
                            due
                        },
                    }
                    .through(&calendar, Some(row.last)),
                );
            }
        }

        let settlements = self.market.short_interest_settlements().await?;
        let horizon = self
            .market
            .calendar(
                FIRST_SETTLEMENT,
                today + buizz_core::visibility::CALENDAR_HORIZON,
            )
            .await?;
        let published: Vec<NaiveDate> = settlement_dates(&calendar, today)
            .into_iter()
            .filter(|settlement| {
                short_interest_published(*settlement, &horizon).is_some_and(|day| day <= today)
            })
            .collect();
        measures.push(
            Scope {
                dataset: Dataset::ShortInterest,
                source: DataSource::Finra,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Settlements,
                scope: "every published FINRA settlement".into(),
                from: FIRST_SETTLEMENT,
                to: published.last().copied().unwrap_or(FIRST_SETTLEMENT),
            }
            .items(published.iter().map(|settlement| {
                if settlements.contains(settlement) {
                    Item::Done
                } else {
                    Item::Pending
                }
            })),
        );
        measures.push(
            Scope {
                dataset: Dataset::ShortVolume,
                source: DataSource::Finra,
                kind: ScopeKind::Indexed,
                unit: ScopeUnit::Sessions,
                scope: "every FINRA daily short volume file".into(),
                from: FIRST_SHORT_VOLUME,
                to: due,
            }
            .through(
                &calendar,
                series(Dataset::ShortVolume, DataSource::Finra).map(|row| row.last),
            ),
        );
        Ok(measures)
    }
}
