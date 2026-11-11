use serde_json::Value;
use tokio::process::Command;

use crate::spine::{PASSWORD, Spine};
use crate::world::repository;

pub async fn drive(script: &str, spine: &Spine, vars: &[(&str, String)]) -> Value {
    let output = Command::new("node")
        .arg(repository().join(format!("e2e/browser/{script}.ts")))
        .env("BUIZZ_URL", &spine.base)
        .env("BUIZZ_PASSWORD", PASSWORD)
        .env("FAKE_URL", &spine.fake.url)
        .envs(vars.iter().map(|(name, value)| (name, value)))
        .output()
        .await
        .expect("node runs the browser script");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{script}.ts failed\n{stdout}\n{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        spine.log()
    );
    stdout
        .lines()
        .last()
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or_default()
}
