use buizz_core::market::{Fact, FactStatus, ModelSpend, RecipeSection};
use buizz_core::time::Timestamp;
use buizz_sources::anthropic::{self, AnthropicClient, ClaudeError, Prompt, Reply, Retry};
use rust_decimal::Decimal;

use super::gold::{self, GoldReport};
use super::{Answer, Version, accept};

pub const MAX_SECTION_CHARS: usize = 300_000;
const MAX_FAILURE_CHARS: usize = 2_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd: Decimal,
}

impl Tally {
    pub fn add(&mut self, reply: &Reply) {
        self.calls += 1;
        self.input_tokens += reply.usage.input_tokens
            + reply.usage.cache_creation_input_tokens
            + reply.usage.cache_read_input_tokens;
        self.output_tokens += reply.usage.output_tokens;
        self.usd += reply.cost;
    }

    pub fn spend(&self, version: &Version) -> Option<ModelSpend> {
        (self.calls > 0).then(|| ModelSpend {
            recipe: version.recipe.name.to_owned(),
            version: version.recipe.version,
            spent_at: Timestamp::now(),
            calls: self.calls,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            usd: self.usd,
        })
    }
}

pub(crate) fn unreadable(section: &RecipeSection) -> Option<String> {
    match &section.text {
        Err(reason) => Some(reason.clone()),
        Ok(text) if text.chars().count() > MAX_SECTION_CHARS => Some(format!(
            "the text is longer than {MAX_SECTION_CHARS} characters"
        )),
        Ok(_) => None,
    }
}

pub(crate) fn judge(
    version: &Version,
    section: &RecipeSection,
    text: &str,
    answer: &anthropic::Answer,
) -> (Vec<Fact>, Option<String>) {
    let (raw, given) = match answer {
        anthropic::Answer::Json(value) => {
            let raw = value.to_string();
            let facts = accept(version, section, text, Answer::Given(&raw));
            (Some(raw), facts)
        }
        anthropic::Answer::Malformed { text: raw } => {
            let facts = accept(version, section, text, Answer::Given(raw));
            (Some(raw.clone()), facts)
        }
        anthropic::Answer::Refused { category } => {
            let reason = format!(
                "the model refused to answer ({})",
                category.as_deref().unwrap_or("no category")
            );
            return (accept(version, section, text, Answer::Failed(reason)), None);
        }
        anthropic::Answer::Truncated => {
            let reason = "the answer was cut off at the token limit".to_owned();
            return (accept(version, section, text, Answer::Failed(reason)), None);
        }
    };
    let failed = given.iter().any(|fact| fact.status == FactStatus::Failed);
    (given, raw.filter(|_| failed))
}

pub(crate) fn failure(facts: &[Fact]) -> String {
    let mut reasons: Vec<&str> = facts
        .iter()
        .filter(|fact| fact.status == FactStatus::Failed)
        .filter_map(|fact| fact.reason.as_deref())
        .collect();
    reasons.dedup();
    reasons.join("; ").chars().take(MAX_FAILURE_CHARS).collect()
}

pub(crate) async fn retry(
    client: &AnthropicClient,
    prompt: &Prompt,
    version: &Version,
    section: &RecipeSection,
    text: &str,
    first: (Vec<Fact>, Option<String>),
    tally: &mut Tally,
) -> Result<Vec<Fact>, ClaudeError> {
    let (facts, Some(answer)) = first else {
        return Ok(first.0);
    };
    let failure = failure(&facts);
    let reply = client
        .message(
            prompt,
            text,
            Some(&Retry {
                answer: &answer,
                failure: &failure,
            }),
        )
        .await?;
    tally.add(&reply);
    Ok(judge(version, section, text, &reply.answer).0)
}

pub async fn ask(
    client: &AnthropicClient,
    version: &Version,
    section: &RecipeSection,
    tally: &mut Tally,
) -> Result<Vec<Fact>, ClaudeError> {
    let text = match (&section.text, unreadable(section)) {
        (Ok(text), None) => text,
        (_, reason) => {
            return Ok(accept(
                version,
                section,
                "",
                Answer::Skipped(reason.unwrap_or_default()),
            ));
        }
    };
    let prompt = version.prompt().expect("a model recipe has a prompt");
    let reply = client.message(&prompt, text, None).await?;
    tally.add(&reply);
    let first = judge(version, section, text, &reply.answer);
    retry(client, &prompt, version, section, text, first, tally).await
}

pub async fn trial(
    client: &AnthropicClient,
    version: &Version,
    split: &str,
    tally: &mut Tally,
) -> Result<GoldReport, String> {
    let documents = gold::documents(version.recipe.name, split)?;
    let mut answers = Vec::with_capacity(documents.len());
    for document in &documents {
        answers.push(
            ask(client, version, &document.section, tally)
                .await
                .map_err(|error| error.to_string())?,
        );
    }
    let mut report = gold::score(version, &documents, &answers, split);
    report.usd = tally.usd;
    Ok(report)
}
