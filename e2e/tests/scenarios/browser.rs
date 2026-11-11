use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::spine::Spine;
use crate::world::repository;

const DRIVER_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn drive(script: &str, spine: &Spine, vars: &[(&str, String)]) -> Value {
    let driver = Command::new("node")
        .arg(repository().join(format!("e2e/browser/{script}.ts")))
        .env("BUIZZ_URL", &spine.base)
        .env("BUIZZ_PASSWORD", &spine.password)
        .env("FAKE_URL", &spine.fake.url)
        .envs(vars.iter().map(|(name, value)| (name, value)))
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(DRIVER_TIMEOUT, driver)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{script}.ts did not finish within {} s\n{}",
                DRIVER_TIMEOUT.as_secs(),
                spine.log()
            )
        })
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
