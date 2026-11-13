use std::collections::BTreeMap;
use std::sync::Arc;

use buizz_core::market::{
    Dataset, Fact, FactStatus, ModelBatch, ModelRequest, RecipeSection, RecipeState, RecipeVersion,
};
use buizz_core::time::Timestamp;
use buizz_sources::anthropic::{
    AnthropicClient, BatchOutcome, BatchRequest, BatchStatus, ClaudeError,
};
use chrono::{Datelike, TimeDelta};
use rust_decimal::Decimal;

use super::model::{Tally, judge, retry, unreadable};
use super::{Answer, Producer, RECIPES, Recipe, Version, accept, ask, trial};
use crate::fetch::{Batch, Work};
use crate::{Ingest, IngestError};

const SECTIONS_PER_BATCH: usize = 200;
const BATCHES_PER_RUN: usize = 25;
const RECENT: TimeDelta = TimeDelta::days(3);
const MESSAGES_PER_RUN: usize = 200;
const MESSAGES_PER_WRITE: usize = 25;
const REQUESTS_PER_BATCH: usize = 2_000;
const BATCH_BYTES: usize = 100 << 20;
const OPEN_BATCHES: usize = 10;
const STOP_PERCENT: u32 = 2;
const GATE_SPLIT: &str = "test";

impl Ingest {
    pub(crate) async fn recipe_facts(&self, now: Timestamp) -> Result<Work, IngestError> {
        if self.market.text_volumes().volumes().is_empty() {
            return Ok(Work::unconfigured("text volumes (BUIZZ_TEXT_DIRS)"));
        }
        let mut batch = Batch::default();
        let mut notes = Vec::new();
        for recipe in &RECIPES {
            let version = Arc::new(Version::new(recipe).map_err(IngestError::Recipe)?);
            let stored = self
                .market
                .register_recipe(recipe.dataset, version.stored())
                .await?;
            match recipe.producer {
                Producer::Code(produce) => {
                    self.code_recipe(recipe, &version, stored, produce, &mut batch)
                        .await?;
                }
                Producer::Model { .. } => match &self.sources.anthropic {
                    Some(client) => {
                        let note = self
                            .model_recipe(recipe, &version, stored, client, now, &mut batch)
                            .await?;
                        notes.push(format!("{}: {note}", recipe.name));
                    }
                    None => notes.push(format!(
                        "{}: Claude (ANTHROPIC_API_KEY) is not configured",
                        recipe.name
                    )),
                },
            }
        }
        let mut work = batch.finish("recipe facts");
        if !notes.is_empty() {
            work.message = format!("{}; {}", work.message, notes.join("; "));
        }
        Ok(work)
    }

    async fn code_recipe(
        &self,
        recipe: &Recipe,
        version: &Arc<Version>,
        stored: RecipeVersion,
        produce: fn(&str) -> serde_json::Value,
        batch: &mut Batch,
    ) -> Result<(), IngestError> {
        if !fills(stored.state) {
            return Ok(());
        }
        for _ in 0..BATCHES_PER_RUN {
            if self.stopping() {
                batch.more_remains(format!("{} sections", recipe.name));
                return Ok(());
            }
            let sections = self
                .market
                .unread_sections(stored.clone(), SECTIONS_PER_BATCH)
                .await?;
            if sections.is_empty() {
                return self.promote(recipe.dataset, stored).await;
            }
            let version = Arc::clone(version);
            let facts = tokio::task::spawn_blocking(move || {
                sections
                    .iter()
                    .flat_map(|section| match &section.text {
                        Ok(text) => accept(
                            &version,
                            section,
                            text,
                            Answer::Given(&produce(text).to_string()),
                        ),
                        Err(reason) => {
                            accept(&version, section, "", Answer::Skipped(reason.clone()))
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
            batch.success(
                self.market
                    .append_facts(recipe.dataset, facts, None, None)
                    .await?
                    .rows,
            );
        }
        batch.more_remains(format!("{} sections", recipe.name));
        Ok(())
    }

    async fn model_recipe(
        &self,
        recipe: &Recipe,
        version: &Version,
        stored: RecipeVersion,
        client: &AnthropicClient,
        now: Timestamp,
        batch: &mut Batch,
    ) -> Result<String, IngestError> {
        let committed = self.market.committed_spend(month_start(now)).await?;
        let budget = client.budget((client.cap() - committed).max(Decimal::ZERO));
        let note = match stored.state {
            RecipeState::Evaluating if stored.gold.is_some() => {
                "evaluating: its gold report missed the targets, so it is never filled".to_owned()
            }
            RecipeState::Evaluating => match self.gate(recipe, version, &budget).await? {
                Ok(note) => {
                    let filling = RecipeVersion {
                        state: RecipeState::Filling,
                        ..stored
                    };
                    let filled = self
                        .fill(recipe, version, filling, &budget, now, batch)
                        .await?;
                    format!("{note}; {filled}")
                }
                Err(note) => note,
            },
            state if fills(state) => {
                self.fill(recipe, version, stored, &budget, now, batch)
                    .await?
            }
            _ => "retired".to_owned(),
        };
        Ok(format!(
            "{note}; ${} of ${} committed this month",
            (committed + budget.spend().spent).round_dp(2),
            client.cap()
        ))
    }

    async fn gate(
        &self,
        recipe: &Recipe,
        version: &Version,
        client: &AnthropicClient,
    ) -> Result<Result<String, String>, IngestError> {
        let mut tally = Tally::default();
        let outcome = trial(client, version, GATE_SPLIT, &mut tally).await;
        self.market
            .append_facts(recipe.dataset, Vec::new(), tally.spend(version), None)
            .await?;
        let report = match outcome {
            Ok(report) => report,
            Err(error) => return Ok(Err(format!("its gold set could not be run: {error}"))),
        };
        let passed = report.passed;
        let missed: Vec<String> = report
            .measures
            .iter()
            .filter(|measure| !measure.passed)
            .map(|measure| format!("{} {}/{}", measure.measure, measure.hits, measure.total))
            .collect();
        self.market
            .advance_recipe(
                recipe.dataset,
                recipe.name.to_owned(),
                recipe.version,
                if passed {
                    RecipeState::Filling
                } else {
                    RecipeState::Evaluating
                },
                Some(
                    serde_json::to_string(&report)
                        .map_err(|error| IngestError::Recipe(error.to_string()))?,
                ),
            )
            .await?;
        Ok(if passed {
            Ok(format!(
                "its gold report passed on {} documents, filling",
                report.documents
            ))
        } else {
            Err(format!(
                "evaluating: its gold report missed {}",
                missed.join(", ")
            ))
        })
    }

    async fn fill(
        &self,
        recipe: &Recipe,
        version: &Version,
        stored: RecipeVersion,
        client: &AnthropicClient,
        now: Timestamp,
        batch: &mut Batch,
    ) -> Result<String, IngestError> {
        let mut notes = Vec::new();
        let batches = self
            .market
            .model_batches(recipe.name.to_owned(), recipe.version)
            .await?;
        let mut open = 0;
        for remote in batches.iter().filter(|remote| remote.open) {
            match self.collect(recipe, version, &stored, client, remote).await {
                Ok(Some(rows)) => batch.success(rows),
                Ok(None) => open += 1,
                Err(error) => {
                    open += 1;
                    batch.failure(&remote.remote_id, &error);
                }
            }
        }
        let collected = self
            .market
            .model_batches(recipe.name.to_owned(), recipe.version)
            .await?;
        if let Some(stalled) = collected
            .iter()
            .rev()
            .find(|remote| !remote.open)
            .filter(|remote| remote.failed.unwrap_or(0) * 100 > remote.requests * STOP_PERCENT)
        {
            return Ok(format!(
                "stopped: {} of {} sections failed in batch {}",
                stalled.failed.unwrap_or(0),
                stalled.requests,
                stalled.remote_id
            ));
        }
        let mut asked = 0;
        let drained = loop {
            if self.stopping() {
                break false;
            }
            let unread = self
                .market
                .unread_sections(stored.clone(), REQUESTS_PER_BATCH)
                .await?;
            if unread.is_empty() {
                break true;
            }
            let (recent, history): (Vec<RecipeSection>, Vec<RecipeSection>) = unread
                .into_iter()
                .partition(|section| section.published_at >= now - RECENT);
            if !recent.is_empty() {
                if asked >= MESSAGES_PER_RUN {
                    break false;
                }
                asked += recent.len();
                if let Err(error) = self
                    .answer_recent(recipe, version, client, recent, batch)
                    .await
                {
                    notes.push(error.to_string());
                    break false;
                }
                continue;
            }
            let (skipped, readable): (Vec<RecipeSection>, Vec<RecipeSection>) = history
                .into_iter()
                .partition(|section| unreadable(section).is_some());
            let skipped: Vec<Fact> = skipped
                .iter()
                .flat_map(|section| {
                    accept(
                        version,
                        section,
                        "",
                        Answer::Skipped(unreadable(section).unwrap_or_default()),
                    )
                })
                .collect();
            if !skipped.is_empty() {
                batch.success(
                    self.market
                        .append_facts(recipe.dataset, skipped, None, None)
                        .await?
                        .rows,
                );
            }
            if readable.is_empty() {
                continue;
            }
            if open >= OPEN_BATCHES {
                break true;
            }
            match self.submit(recipe, version, client, now, readable).await {
                Ok(_) => open += 1,
                Err(error) => {
                    notes.push(error.to_string());
                    break false;
                }
            }
        };
        if !drained {
            batch.more_remains(format!("{} sections", recipe.name));
        }
        if open == 0 && drained {
            self.promote(recipe.dataset, stored).await?;
        }
        notes.insert(0, format!("{open} batches open"));
        Ok(notes.join("; "))
    }

    async fn answer_recent(
        &self,
        recipe: &Recipe,
        version: &Version,
        client: &AnthropicClient,
        recent: Vec<RecipeSection>,
        batch: &mut Batch,
    ) -> Result<(), ClaudeError> {
        for chunk in recent.chunks(MESSAGES_PER_WRITE) {
            let mut tally = Tally::default();
            let mut facts = Vec::new();
            let mut stopped = None;
            for section in chunk {
                if self.stopping() {
                    break;
                }
                match ask(client, version, section, &mut tally).await {
                    Ok(answered) => facts.extend(answered),
                    Err(error) => {
                        stopped = Some(error);
                        break;
                    }
                }
            }
            let written = self
                .market
                .append_facts(recipe.dataset, facts, tally.spend(version), None)
                .await;
            match written {
                Ok(summary) => batch.success(summary.rows),
                Err(error) => batch.failure(recipe.name, &error),
            }
            if let Some(error) = stopped {
                return Err(error);
            }
        }
        Ok(())
    }

    async fn submit(
        &self,
        recipe: &Recipe,
        version: &Version,
        client: &AnthropicClient,
        now: Timestamp,
        sections: Vec<RecipeSection>,
    ) -> Result<(), IngestError> {
        let prompt = version.prompt().expect("a model recipe has a prompt");
        let mut bytes = 0;
        let (requests, documents): (Vec<ModelRequest>, Vec<BatchRequest>) = sections
            .into_iter()
            .take_while(|section| {
                bytes += section.text.as_ref().map_or(0, String::len);
                bytes <= BATCH_BYTES
            })
            .enumerate()
            .filter_map(|(index, section)| {
                let document = section.text.ok()?;
                let custom_id = format!("s{index}");
                Some((
                    ModelRequest {
                        custom_id: custom_id.clone(),
                        doc_id: section.doc_id,
                        section_key: section.section_key,
                    },
                    BatchRequest {
                        custom_id,
                        document,
                    },
                ))
            })
            .unzip();
        let reserved = documents
            .iter()
            .map(|request| prompt.worst_case(&request.document, true))
            .sum();
        let created = client
            .create_batch(&prompt, &documents)
            .await
            .map_err(|error| IngestError::Recipe(error.to_string()))?;
        let count = requests.len();
        self.market
            .submit_model_batch(
                recipe.dataset,
                ModelBatch {
                    recipe: recipe.name.to_owned(),
                    version: recipe.version,
                    remote_id: created.id,
                    open: true,
                    requests: u32::try_from(count).unwrap_or(u32::MAX),
                    failed: None,
                    reserved,
                    at: now,
                },
                requests,
            )
            .await?;
        Ok(())
    }

    async fn collect(
        &self,
        recipe: &Recipe,
        version: &Version,
        stored: &RecipeVersion,
        client: &AnthropicClient,
        remote: &ModelBatch,
    ) -> Result<Option<u64>, IngestError> {
        let failed = |error: ClaudeError| IngestError::Recipe(error.to_string());
        let status = client.batch(&remote.remote_id).await.map_err(failed)?;
        if status.processing_status != BatchStatus::Ended {
            return Ok(None);
        }
        let (requests, sections) = self
            .market
            .requested_sections(stored.clone(), remote.remote_id.clone())
            .await?;
        let by_key: BTreeMap<(&str, &str), &RecipeSection> = sections
            .iter()
            .map(|section| {
                (
                    (section.doc_id.as_str(), section.section_key.as_str()),
                    section,
                )
            })
            .collect();
        let requested: BTreeMap<&str, &RecipeSection> = requests
            .iter()
            .filter_map(|request| {
                by_key
                    .get(&(request.doc_id.as_str(), request.section_key.as_str()))
                    .map(|section| (request.custom_id.as_str(), *section))
            })
            .collect();
        let prompt = version.prompt().expect("a model recipe has a prompt");
        let mut results = client
            .batch_results(&remote.remote_id)
            .await
            .map_err(failed)?;
        let mut tally = Tally::default();
        let mut facts = Vec::new();
        let mut failed_sections = 0u32;
        while let Some(result) = results.next().await {
            let result = result.map_err(failed)?;
            let Some(section) = requested.get(result.custom_id.as_str()) else {
                continue;
            };
            let Ok(text) = &section.text else {
                continue;
            };
            let answered = match result.outcome {
                BatchOutcome::Answered(reply) => {
                    tally.add(&reply);
                    let first = judge(version, section, text, &reply.answer);
                    match retry(client, &prompt, version, section, text, first, &mut tally).await {
                        Ok(answered) => answered,
                        Err(_) => continue,
                    }
                }
                BatchOutcome::Errored {
                    kind,
                    retryable: false,
                } => accept(
                    version,
                    section,
                    text,
                    Answer::Failed(format!("the API rejected the request: {kind}")),
                ),
                _ => continue,
            };
            failed_sections += u32::from(
                answered
                    .iter()
                    .any(|fact| fact.status == FactStatus::Failed),
            );
            facts.extend(answered);
        }
        let summary = self
            .market
            .append_facts(
                recipe.dataset,
                facts,
                tally.spend(version),
                Some(ModelBatch {
                    open: false,
                    failed: Some(failed_sections),
                    at: Timestamp::now(),
                    ..remote.clone()
                }),
            )
            .await?;
        Ok(Some(summary.rows))
    }

    async fn promote(&self, dataset: Dataset, stored: RecipeVersion) -> Result<(), IngestError> {
        let progress = self.market.recipe_progress(stored.clone()).await?;
        if progress.done + progress.failed < progress.expected {
            return Ok(());
        }
        let steps: &[RecipeState] = match stored.state {
            RecipeState::Filling => &[RecipeState::Complete, RecipeState::Current],
            RecipeState::Complete => &[RecipeState::Current],
            _ => &[],
        };
        for step in steps {
            self.market
                .advance_recipe(dataset, stored.recipe.clone(), stored.version, *step, None)
                .await?;
        }
        Ok(())
    }
}

fn fills(state: RecipeState) -> bool {
    matches!(
        state,
        RecipeState::Filling | RecipeState::Complete | RecipeState::Current
    )
}

fn month_start(now: Timestamp) -> Timestamp {
    let date = now
        .to_datetime()
        .date_naive()
        .with_day(1)
        .unwrap_or_default();
    Timestamp::from_datetime(date.and_time(chrono::NaiveTime::MIN).and_utc()).unwrap_or(now)
}
