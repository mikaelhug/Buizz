use buizz_core::time::Timestamp;
use rusqlite::{Transaction, params};

use crate::Result;

pub fn record_study(
    transaction: &Transaction,
    label: &str,
    hash: &str,
    events: u32,
    at: Timestamp,
) -> Result<u32> {
    transaction.execute(
        "INSERT INTO event_study (label, hash, events, created_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT (label, hash) DO NOTHING",
        params![label, hash, events, at.nanos()],
    )?;
    Ok(transaction.query_row("SELECT COUNT(*) FROM event_study", [], |row| row.get(0))?)
}
