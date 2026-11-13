use anyhow::{Context, Result, bail};
use buizz_ingest::recipe::{Tally, Version, recipe, trial};
use buizz_sources::anthropic::{self, AnthropicClient, AnthropicConfig};
use rust_decimal::Decimal;

const USAGE: &str = "usage: recipe-trial <recipe> <dev|test> [max dollars, default 5]";

#[tokio::main]
async fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (name, split, cap) = match arguments.as_slice() {
        [name, split] => (name, split, anthropic::DEFAULT_SPEND_CAP),
        [name, split, cap] => (name, split, cap.parse::<Decimal>().context(USAGE)?),
        _ => bail!(USAGE),
    };
    if !["dev", "test"].contains(&split.as_str()) {
        bail!(USAGE);
    }
    let recipe = recipe(name).with_context(|| format!("no recipe named {name}"))?;
    let version = Version::new(recipe).map_err(anyhow::Error::msg)?;
    let api_key = std::env::var(anthropic::KEY_ENV)
        .with_context(|| format!("{} must be set", anthropic::KEY_ENV))?;
    let client = AnthropicClient::new(AnthropicConfig {
        base_url: std::env::var(anthropic::URL_ENV)
            .unwrap_or_else(|_| anthropic::BASE_URL.to_owned()),
        spend_cap: cap,
        ..AnthropicConfig::new(api_key)
    })?;
    let mut tally = Tally::default();
    let report = trial(&client, &version, split, &mut tally)
        .await
        .map_err(anyhow::Error::msg)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    eprintln!(
        "{} calls, {} input and {} output tokens, ${}; {}",
        tally.calls,
        tally.input_tokens,
        tally.output_tokens,
        tally.usd.round_dp(4),
        if report.passed {
            "passed"
        } else {
            "missed a target"
        }
    );
    Ok(())
}
