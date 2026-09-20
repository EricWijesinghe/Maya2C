//! Observe, decide, apply.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::error::Result;
use crate::policy::decide;
use crate::sink::{FirewallSink, SinkAction};
use crate::source::ThreatSource;

/// What one tick did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepReport {
    /// The height indicators were judged at.
    pub height: u64,
    /// Addresses newly dropped.
    pub blocked: Vec<IpAddr>,
    /// Addresses no longer dropped.
    pub unblocked: Vec<IpAddr>,
    /// Changes the sink refused, retried next tick.
    pub failures: Vec<String>,
}

/// The per-host worker.
pub struct Worker {
    source: Arc<dyn ThreatSource>,
    sink: Box<dyn FirewallSink>,
    enforced: BTreeSet<IpAddr>,
}

impl Worker {
    /// A worker that has enforced nothing yet.
    #[must_use]
    pub fn new(source: Arc<dyn ThreatSource>, sink: Box<dyn FirewallSink>) -> Self {
        Self {
            source,
            sink,
            enforced: BTreeSet::new(),
        }
    }

    /// Addresses currently dropped, by this worker's own record.
    #[must_use]
    pub fn enforced(&self) -> &BTreeSet<IpAddr> {
        &self.enforced
    }

    /// One tick. A sink failure is reported, not raised, and the address stays
    /// in whichever state it was so the next tick retries.
    ///
    /// # Errors
    ///
    /// Only if the source cannot be read; nothing is changed then.
    pub async fn step(&mut self) -> Result<StepReport> {
        let observation = self.source.observe().await?;
        let plan = decide(&observation, &self.enforced);
        let mut report = StepReport {
            height: observation.height,
            ..StepReport::default()
        };
        for ip in plan.block {
            match self.sink.apply(SinkAction::Block(ip)) {
                Ok(()) => {
                    self.enforced.insert(ip);
                    report.blocked.push(ip);
                }
                Err(error) => report.failures.push(error.to_string()),
            }
        }
        for ip in plan.unblock {
            match self.sink.apply(SinkAction::Unblock(ip)) {
                Ok(()) => {
                    self.enforced.remove(&ip);
                    report.unblocked.push(ip);
                }
                Err(error) => report.failures.push(error.to_string()),
            }
        }
        Ok(report)
    }

    /// Ticks every `poll` until `shutdown` flips, handing each outcome to
    /// `on_step`.
    pub async fn run(
        &mut self,
        poll: Duration,
        mut shutdown: watch::Receiver<bool>,
        mut on_step: impl FnMut(Result<StepReport>),
    ) {
        let mut ticker = tokio::time::interval(poll);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => on_step(self.step().await),
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::net::Ipv4Addr;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use maya_threat_intel::{OffenceKind, ThreatIndicator};

    use super::*;
    use crate::error::FirewallError;
    use crate::source::Observation;

    struct Fixed(Mutex<Observation>);

    #[async_trait]
    impl ThreatSource for Fixed {
        async fn observe(&self) -> Result<Observation> {
            Ok(self.0.lock().expect("observation").clone())
        }
    }

    /// Refuses the first block it is asked for, then complies.
    #[derive(Default)]
    struct Flaky {
        refused: bool,
    }

    impl FirewallSink for Flaky {
        fn apply(&mut self, _action: SinkAction) -> Result<()> {
            if self.refused {
                return Ok(());
            }
            self.refused = true;
            Err(FirewallError::Sink("busy".to_owned()))
        }
    }

    #[tokio::test]
    async fn a_refused_block_is_retried_on_the_next_tick() {
        let offender = [9; 32];
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
        let source = Arc::new(Fixed(Mutex::new(Observation {
            height: 5,
            indicators: vec![(
                offender,
                ThreatIndicator::observe(None, OffenceKind::TxRootMismatch, 5),
            )],
            addresses: vec![(offender, ip)],
        })));
        let mut worker = Worker::new(source, Box::new(Flaky::default()));

        let first = worker.step().await.expect("observed");
        assert!(first.blocked.is_empty());
        assert_eq!(first.failures.len(), 1);
        assert!(worker.enforced().is_empty());

        let second = worker.step().await.expect("observed");
        assert_eq!(second.blocked, vec![ip]);
        assert_eq!(worker.enforced(), &BTreeSet::from([ip]));
    }
}
