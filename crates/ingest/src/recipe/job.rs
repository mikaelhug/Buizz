use std::sync::Arc;

use buizz_core::market::{Dataset, Fact, RecipeSection, RecipeState, RecipeVersion};

use super::{Answer, Producer, RECIPES, Version, accept};
use crate::fetch::{Batch, Work};
use crate::{Ingest, IngestError};

const SECTIONS_PER_BATCH: usize = 200;
const BATCHES_PER_RUN: usize = 25;
const NO_TEXT: &str = "the document has no text";

impl Ingest {
    pub(crate) async fn recipe_facts(&self) -> Result<Work, IngestError> {
        if self.market.text_volumes().volumes().is_empty() {
            return Ok(Work::unconfigured("text volumes (BUIZZ_TEXT_DIRS)"));
        }
        let mut batch = Batch::default();
        for recipe in &RECIPES {
            let version = Arc::new(Version::new(recipe).map_err(IngestError::Recipe)?);
            let stored = self
                .market
                .register_recipe(recipe.dataset, version.stored())
                .await?;
            let Producer::Code(produce) = recipe.producer else {
                continue;
            };
            if !matches!(
                stored.state,
                RecipeState::Filling | RecipeState::Complete | RecipeState::Current
            ) {
                continue;
            }
            let mut drained = false;
            for _ in 0..BATCHES_PER_RUN {
                if self.stopping() {
                    break;
                }
                let sections = self
                    .market
                    .unread_sections(stored.clone(), SECTIONS_PER_BATCH)
                    .await?;
                if sections.is_empty() {
                    drained = true;
                    break;
                }
                let version = Arc::clone(&version);
                let facts = tokio::task::spawn_blocking(move || {
                    sections
                        .iter()
                        .flat_map(|section| answer(&version, produce, section))
                        .collect::<Vec<_>>()
                })
                .await
                .map_err(|error| std::io::Error::other(error.to_string()))?;
                batch.success(self.market.append_facts(recipe.dataset, facts).await?.rows);
            }
            if drained {
                self.promote(recipe.dataset, stored).await?;
            } else {
                batch.more_remains(format!("{} sections", recipe.name));
            }
        }
        Ok(batch.finish("recipe facts"))
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

fn answer(
    version: &Version,
    produce: fn(&str) -> serde_json::Value,
    section: &RecipeSection,
) -> Vec<Fact> {
    match &section.text {
        Some(text) => accept(
            version,
            section,
            text,
            Answer::Given(&produce(text).to_string()),
        ),
        None => accept(version, section, "", Answer::Skipped(NO_TEXT.to_owned())),
    }
}
