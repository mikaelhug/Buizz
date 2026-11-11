use buizz_state::{StateError, StateStore, StepCommit};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const QUEUE: usize = 256;

pub(crate) struct Committer {
    queue: mpsc::Sender<StepCommit>,
    task: Option<JoinHandle<Result<(), StateError>>>,
}

impl Committer {
    pub(crate) fn spawn(state: StateStore, last_step: u64) -> Self {
        let (queue, mut commits) = mpsc::channel::<StepCommit>(QUEUE);
        let task = tokio::spawn(async move {
            let mut step_seq = last_step;
            while let Some(mut commit) = commits.recv().await {
                while let Ok(next) = commits.try_recv() {
                    merge(&mut commit, next);
                }
                step_seq += 1;
                commit.step_seq = step_seq;
                state.commit_step(commit).await?;
            }
            Ok(())
        });
        Self {
            queue,
            task: Some(task),
        }
    }

    pub(crate) async fn send(&mut self, commit: StepCommit) -> Result<(), StateError> {
        if self.queue.send(commit).await.is_ok() {
            return Ok(());
        }
        Err(finished(self.task.take())
            .await
            .err()
            .unwrap_or(StateError::Closed))
    }

    pub(crate) async fn finish(self) -> Result<(), StateError> {
        drop(self.queue);
        finished(self.task).await
    }
}

async fn finished(task: Option<JoinHandle<Result<(), StateError>>>) -> Result<(), StateError> {
    match task {
        Some(task) => task.await.unwrap_or(Err(StateError::Closed)),
        None => Err(StateError::Closed),
    }
}

fn merge(into: &mut StepCommit, next: StepCommit) {
    into.clock = next.clock;
    into.equity = next.equity;
    into.ledger.extend(next.ledger);
    into.orders.extend(next.orders);
    into.equity_points.extend(next.equity_points);
    into.events.extend(next.events);
    into.engine_checkpoint = next.engine_checkpoint;
    if next.bot_state.is_some() {
        into.bot_state = next.bot_state;
    }
}
