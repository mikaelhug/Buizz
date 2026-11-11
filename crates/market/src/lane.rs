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
                let _ = reply.send(work(connection));
            }))
            .await
            .map_err(|_| MarketError::Closed)?;
        response.await.map_err(|_| MarketError::Closed)?
    }
}
