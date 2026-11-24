//! Gossiped transactions are checked off the swarm task (audit BFT-06).
//!
//! A transaction check reads state and verifies a post-quantum signature
//! (`Mempool::insert`). Run inline, a flood of valid-looking transactions held
//! up every other message the swarm task delivers, DAG-BFT votes included,
//! which is a liveness risk. Gossipsub validation is already asynchronous
//! (`validate_messages`): nothing is relayed until a verdict is reported,
//! however late, so moving the check changes when the verdict comes, never
//! what is relayed unchecked.
//!
//! The backlog is bounded in count and bytes. A transaction that finds it full
//! is ignored: not relayed by this node and not held against its sender, since
//! overload is not the sender's fault. The mesh's other peers still relay it.

use std::collections::VecDeque;
use std::sync::Arc;

use libp2p::PeerId;
use libp2p::gossipsub::MessageId;
use tokio::task::{JoinError, JoinSet};

use crate::error::Result;

/// Checks running at once on the blocking pool. Each is CPU-bound, so more
/// than a few only contend with block execution for cores.
const TX_CHECK_WORKERS: usize = 4;

/// Transactions waiting for a worker.
const MAX_TX_CHECKS_WAITING: usize = 1024;

/// Wire bytes waiting in all. A count alone would let 1,024 maximum-size
/// transactions pin 4 GiB.
const MAX_TX_CHECK_BYTES: usize = 32 * 1024 * 1024;

/// One gossiped transaction and what reporting its verdict needs.
pub(super) struct TxJob {
    pub(super) source: PeerId,
    pub(super) id: MessageId,
    pub(super) author: Option<PeerId>,
    pub(super) sequence_number: Option<u64>,
    /// The author's gossip signature, kept for evidence (`node::threat`).
    pub(super) signature: Vec<u8>,
    pub(super) data: Vec<u8>,
}

/// The check itself: in the node, `Mempool::insert_encoded`.
pub(super) type Check = Arc<dyn Fn(&[u8]) -> Result<bool> + Send + Sync>;

/// A finished check, or the worker's panic (the job is lost with it, and
/// gossipsub ages the unreported message out unrelayed).
pub(super) type Finished = std::result::Result<(TxJob, Result<bool>), JoinError>;

/// Bounded pool of transaction checks.
pub(super) struct TxChecks {
    check: Check,
    running: JoinSet<(TxJob, Result<bool>)>,
    waiting: VecDeque<TxJob>,
    waiting_bytes: usize,
}

impl TxChecks {
    pub(super) fn new(check: Check) -> Self {
        Self {
            check,
            running: JoinSet::new(),
            waiting: VecDeque::new(),
            waiting_bytes: 0,
        }
    }

    /// Starts or queues `job`. `false` means the backlog is full and the
    /// caller reports the message as ignored.
    pub(super) fn submit(&mut self, job: TxJob) -> bool {
        if self.running.len() < TX_CHECK_WORKERS {
            self.start(job);
            return true;
        }
        let bytes = job.data.len();
        if self.waiting.len() >= MAX_TX_CHECKS_WAITING
            || self.waiting_bytes.saturating_add(bytes) > MAX_TX_CHECK_BYTES
        {
            return false;
        }
        self.waiting_bytes += bytes;
        self.waiting.push_back(job);
        true
    }

    fn start(&mut self, job: TxJob) {
        let check = Arc::clone(&self.check);
        self.running.spawn_blocking(move || {
            let result = check(&job.data);
            (job, result)
        });
    }

    /// The next finished check, starting a waiting one in its place. `None`
    /// at once when nothing runs, which disables a `select!` branch.
    /// Cancel-safe: `JoinSet::join_next` is, and a waiting job starts only
    /// after a completion was taken.
    pub(super) async fn finished(&mut self) -> Option<Finished> {
        let done = self.running.join_next().await?;
        if let Some(job) = self.waiting.pop_front() {
            self.waiting_bytes -= job.data.len();
            self.start(job);
        }
        Some(done)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    fn job(data: Vec<u8>) -> TxJob {
        TxJob {
            source: PeerId::random(),
            id: MessageId::new(&data),
            author: None,
            sequence_number: None,
            signature: vec![],
            data,
        }
    }

    /// A check that blocks until the returned sender is dropped or sent to.
    fn gated() -> (Check, mpsc::Sender<()>) {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let check: Check = Arc::new(move |_| {
            let _ = gate.lock().unwrap().recv();
            Ok(true)
        });
        (check, release)
    }

    #[tokio::test]
    async fn a_stuck_check_does_not_hold_up_the_caller() {
        let (check, release) = gated();
        let mut checks = TxChecks::new(check);
        // Every worker is stuck; submitting more still returns at once.
        for i in 0..TX_CHECK_WORKERS + 8 {
            assert!(checks.submit(job(vec![u8::try_from(i).unwrap()])));
        }
        assert_eq!(checks.waiting.len(), 8, "jobs ran past the worker cap");
        drop(release);
        let mut done = 0;
        while let Some(finished) = tokio::time::timeout(Duration::from_secs(30), checks.finished())
            .await
            .expect("a check never finished")
        {
            let (_, result) = finished.unwrap();
            assert!(result.unwrap());
            done += 1;
        }
        assert_eq!(done, TX_CHECK_WORKERS + 8, "a queued check was lost");
        assert_eq!(checks.waiting_bytes, 0);
    }

    #[tokio::test]
    async fn a_full_backlog_refuses_instead_of_growing() {
        let (check, _release) = gated();
        let mut checks = TxChecks::new(check);
        for _ in 0..TX_CHECK_WORKERS + MAX_TX_CHECKS_WAITING {
            assert!(checks.submit(job(vec![0])));
        }
        assert!(
            !checks.submit(job(vec![0])),
            "the backlog grew past its count"
        );
    }

    #[tokio::test]
    async fn the_backlog_is_bounded_in_bytes() {
        let (check, _release) = gated();
        let mut checks = TxChecks::new(check);
        for _ in 0..TX_CHECK_WORKERS {
            assert!(checks.submit(job(vec![])));
        }
        let big = MAX_TX_CHECK_BYTES / 4;
        for _ in 0..4 {
            assert!(checks.submit(job(vec![0; big])));
        }
        assert!(
            !checks.submit(job(vec![0; 1])),
            "the backlog grew past its bytes"
        );
    }
}
