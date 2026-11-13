use duckdb::{Connection, params};

use crate::{MarketError, Result, SCHEMA};

const MIGRATIONS: [&str; 12] = [
    include_str!("../migrations/2_revisions.sql"),
    include_str!("../migrations/3_sec_filings.sql"),
    include_str!("../migrations/4_daily_watermark.sql"),
    include_str!("../migrations/5_insiders_and_news.sql"),
    include_str!("../migrations/6_fomc_meetings.sql"),
    include_str!("../migrations/7_earnings_insight.sql"),
    include_str!("../migrations/8_corporate_action_kinds.sql"),
    include_str!("../migrations/9_quiet_sessions.sql"),
    include_str!("../migrations/10_rebackfill_corporate_actions.sql"),
    include_str!("../migrations/11_current_reports.sql"),
    include_str!("../migrations/12_short_selling.sql"),
    include_str!("../migrations/13_text_store.sql"),
];

pub(crate) const LATEST: i64 = MIGRATIONS.len() as i64 + 1;

pub(crate) fn migrate(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    let fresh: bool = transaction.query_row(
        "SELECT count(*) = 0 FROM duckdb_tables() \
         WHERE database_name = current_database() AND schema_name = 'main' \
         AND table_name <> 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    transaction
        .execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version BIGINT NOT NULL)")?;
    let version: i64 = if fresh {
        transaction.execute_batch(SCHEMA)?;
        LATEST
    } else {
        transaction.query_row(
            "SELECT coalesce(max(version), 1) FROM schema_version",
            [],
            |row| row.get(0),
        )?
    };
    if version > LATEST {
        return Err(MarketError::NewerSchema {
            found: version,
            known: LATEST,
        });
    }
    for migration in MIGRATIONS
        .iter()
        .skip(usize::try_from(version - 1).unwrap_or(0))
    {
        transaction.execute_batch(migration)?;
    }
    transaction.execute_batch("DELETE FROM schema_version")?;
    transaction.execute(
        "INSERT INTO schema_version (version) VALUES (?)",
        params![LATEST],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION_1: &str = include_str!("../tests/fixtures/schema_v1.sql");

    fn catalog(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT table_name || '.' || column_name || ' ' || data_type || ' ' || is_nullable \
                 || ' ' || coalesce(column_default, '') FROM information_schema.columns \
                 WHERE table_schema = 'main' \
                 UNION ALL SELECT table_name || ' ' || constraint_text FROM duckdb_constraints() \
                 WHERE schema_name = 'main' \
                 UNION ALL SELECT 'sequence ' || sequence_name FROM duckdb_sequences() \
                 WHERE schema_name = 'main' \
                 UNION ALL SELECT 'index ' || index_name || ' ' || coalesce(sql, '') FROM duckdb_indexes() \
                 WHERE schema_name = 'main' ORDER BY 1",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<duckdb::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_version_1_store_migrates_to_exactly_the_fresh_schema() {
        let mut fresh = Connection::open_in_memory().unwrap();
        migrate(&mut fresh).unwrap();
        let mut old = Connection::open_in_memory().unwrap();
        old.execute_batch(VERSION_1).unwrap();
        migrate(&mut old).unwrap();
        assert_eq!(catalog(&old), catalog(&fresh));
        migrate(&mut old).unwrap();
        let version: i64 = old
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, LATEST);
    }

    #[test]
    fn symbols_backfilled_before_every_action_kind_was_read_are_backfilled_again() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "UPDATE schema_version SET version = 9; \
                 INSERT INTO coverage VALUES \
                 ('corporate_actions', 'DD', 'alpaca', '2016-01-04', '2026-11-12', now()), \
                 ('corporate_actions', '', 'alpaca', '2016-01-04', '2026-11-12', now()), \
                 ('bars1d', 'DD', 'alpaca', '2016-01-04', '2026-11-12', now())",
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        let mut statement = connection
            .prepare("SELECT dataset || ':' || symbol FROM coverage ORDER BY 1")
            .unwrap();
        let covered: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<duckdb::Result<_>>()
            .unwrap();
        assert_eq!(covered, ["bars1d:DD", "corporate_actions:"]);
    }

    #[test]
    fn a_store_from_a_newer_spine_is_refused() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(&format!(
                "UPDATE schema_version SET version = {}",
                LATEST + 1
            ))
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(MarketError::NewerSchema { .. })
        ));
    }
}
