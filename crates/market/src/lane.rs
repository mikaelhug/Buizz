use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use duckdb::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::{MarketError, Result};

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

const QUEUE_PER_CONNECTION: usize = 16;

#[derive(Clone)]
pub(crate) struct Lane(mpsc::Sender<Job>);

impl Lane {
    pub(crate) fn spawn(name: &str, connections: Vec<Connection>) -> Result<Self> {
        let (sender, receiver) = mpsc::channel::<Job>(connections.len() * QUEUE_PER_CONNECTION);
        let receiver = Arc::new(Mutex::new(receiver));
        for (index, mut connection) in connections.into_iter().enumerate() {
            let receiver = Arc::clone(&receiver);
            thread::Builder::new()
                .name(format!("{name}-{index}"))
                .spawn(move || {
                    loop {
                        let next = receiver
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .blocking_recv();
                        let Some(job) = next else { break };
                        job(&mut connection);
                    }
                })?;
        }
        Ok(Self(sender))
    }

    pub(crate) async fn run<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let (reply, response) = oneshot::channel();
        self.0
            .send(Box::new(move |connection| {
                let result = catch_unwind(AssertUnwindSafe(|| work(connection)))
                    .unwrap_or_else(|panic| Err(panicked(panic.as_ref())));
                let _ = reply.send(result);
            }))
            .await
            .map_err(|_| MarketError::Closed)?;
        response.await.map_err(|_| MarketError::Closed)?
    }
}

fn panicked(panic: &(dyn Any + Send)) -> MarketError {
    let message = panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    tracing::error!(%message, "a market store job panicked; the worker keeps serving");
    MarketError::Panicked(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_panicking_job_fails_alone_and_the_worker_keeps_serving() {
        let lane = Lane::spawn("test", vec![Connection::open_in_memory().unwrap()]).unwrap();
        let failed = lane.run(|_| -> Result<()> { panic!("boom") }).await;
        assert!(matches!(failed, Err(MarketError::Panicked(message)) if message == "boom"));
        let answer = lane
            .run(|connection| {
                Ok(connection.query_row("SELECT 42", [], |row| row.get::<_, i64>(0))?)
            })
            .await;
        assert_eq!(answer.unwrap(), 42);
    }
}
