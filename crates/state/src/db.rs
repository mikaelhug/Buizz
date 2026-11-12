use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use tokio::sync::oneshot;

use crate::{Result, SCHEMA, StateError};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATIONS: [&str; 9] = [
    include_str!("../migrations/1_financing.sql"),
    include_str!("../migrations/2_undelivered.sql"),
    include_str!("../migrations/3_drawdown_badge.sql"),
    include_str!("../migrations/4_partial_jobs.sql"),
    include_str!("../migrations/5_waiting_jobs.sql"),
    include_str!("../migrations/6_market_reset.sql"),
    include_str!("../migrations/7_live_stats.sql"),
    include_str!("../migrations/8_bot_usage.sql"),
    include_str!("../migrations/9_variants.sql"),
];

type WriteJob = Box<dyn FnOnce(&mut Connection) + Send>;

pub struct Db {
    path: PathBuf,
    writer: mpsc::Sender<WriteJob>,
    readers: Arc<Mutex<Vec<Connection>>>,
}

impl Db {
    pub async fn open(path: &Path) -> Result<Self> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut connection = Connection::open(&path)?;
            connection.busy_timeout(BUSY_TIMEOUT)?;
            connection.pragma_update(None, "journal_mode", "WAL")?;
            connection.pragma_update(None, "synchronous", "FULL")?;
            connection.pragma_update(None, "foreign_keys", true)?;
            migrate(&mut connection)?;
            let (writer, jobs) = mpsc::channel::<WriteJob>();
            thread::Builder::new()
                .name("state-writer".into())
                .spawn(move || {
                    let mut connection = connection;
                    for job in jobs {
                        job(&mut connection);
                    }
                })
                .map_err(|_| StateError::Closed)?;
            Ok(Self {
                path,
                writer,
                readers: Arc::default(),
            })
        })
        .await
        .map_err(|_| StateError::Closed)?
    }

    pub async fn transaction<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction) -> Result<T> + Send + 'static,
    {
        let (reply, response) = oneshot::channel();
        self.writer
            .send(Box::new(move |connection| {
                let outcome = (|| {
                    let transaction =
                        connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let value = work(&transaction)?;
                    transaction.commit()?;
                    Ok(value)
                })();
                let _ = reply.send(outcome);
            }))
            .map_err(|_| StateError::Closed)?;
        response.await.map_err(|_| StateError::Closed)?
    }

    pub async fn read<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let path = self.path.clone();
        let readers = Arc::clone(&self.readers);
        tokio::task::spawn_blocking(move || {
            let pooled = readers.lock().map_err(|_| StateError::Closed)?.pop();
            let connection = match pooled {
                Some(connection) => connection,
                None => open_reader(&path)?,
            };
            let outcome = work(&connection);
            if let Ok(mut pool) = readers.lock() {
                pool.push(connection);
            }
            outcome
        })
        .await
        .map_err(|_| StateError::Closed)?
    }
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let fresh: bool = transaction.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'run')",
        [],
        |row| row.get(0),
    )?;
    let latest = MIGRATIONS.len() as i64;
    let applied: i64 = if fresh {
        latest
    } else {
        transaction.pragma_query_value(None, "user_version", |row| row.get(0))?
    };
    if applied > latest {
        return Err(StateError::NewerSchema {
            found: applied,
            known: latest,
        });
    }
    for migration in MIGRATIONS
        .iter()
        .skip(usize::try_from(applied).unwrap_or(0))
    {
        transaction.execute_batch(migration)?;
    }
    transaction.execute_batch(SCHEMA)?;
    transaction.pragma_update(None, "user_version", latest)?;
    transaction.commit()?;
    Ok(())
}

fn open_reader(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(connection)
}
