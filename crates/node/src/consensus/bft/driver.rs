//! The node's end of the engine: frames in, frames and blocks out.
//!
//! Synchronous and transport-free, like the engine it wraps. The binary owns
//! the gossip loop and the clock and calls [`BftDriver::on_frame`] and
//! [`BftDriver::on_tick`]; everything here is testable without a socket.
//!
//! Two rules this file exists to keep:
//!
//! 1. **Nothing this validator signs leaves before it is on disk.** Every own
//!    proposal and vote is fsync'd to the [`SafetyStore`] before its frame is
//!    returned for sending (Master Prompt 16 §2's rule, applied to votes).
//! 2. **Blocks come from certificates, never from peers.** Each committed
//!    sub-DAG is built into a block locally (`builder`) and inserted; an anchor
//!    already turned into a block before a restart is skipped by comparing its
//!    seal with the tip's.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use maya_dag_bft::{
    Certificate, Committee, Dest, Digest, Equivocation, Message, Output, Params, Validator,
    ValidatorId,
};

use super::auth::MlDsaAuthenticator;
use super::builder::{build_block, unseal};
use super::store::SafetyStore;
use super::wire::{BROADCAST, Envelope};
use crate::consensus::{BlockId, Chain, InsertOutcome};
use crate::core::Transaction;
use crate::crypto::keys::{SigningKey, VerifyingKey};
use crate::error::{NodeError, Result};

/// Everything a node needs to run one epoch of DAG-BFT.
#[derive(Clone)]
pub struct BftSetup {
    /// Committee epoch.
    pub epoch: u64,
    /// The committee's verifying keys, in validator-id order.
    pub committee: Arc<[VerifyingKey]>,
    /// This node's signing key, if it is a validator.
    pub signer: Option<Arc<SigningKey>>,
    /// Engine parameters (batch bounds, anchor timeout). Consensus: from
    /// genesis, identical on every node.
    pub params: Params,
}

/// What one step produced for the binary to act on.
#[derive(Debug, Default)]
pub struct Step {
    /// Encoded frames to publish on the BFT topic.
    pub frames: Vec<Vec<u8>>,
    /// Blocks inserted, in order.
    pub blocks: Vec<BlockId>,
    /// Transactions those blocks included, to drop from the mempool.
    pub included: Vec<[u8; 32]>,
    /// Each built block's anchor proposal time, milliseconds: what the
    /// finality-latency metric measures from.
    pub anchor_times_ms: Vec<u64>,
    /// Equivocations this node witnessed, for the staking module.
    pub equivocations: Vec<Equivocation>,
}

/// One epoch's engine, its safety log, and the bookkeeping between them.
pub struct BftDriver {
    engine: Validator<MlDsaAuthenticator>,
    id: Option<ValidatorId>,
    epoch: u64,
    store: SafetyStore,
    /// Certificates already written to the log, so a re-broadcast is not
    /// appended every tick. Pruned with the engine's horizon.
    logged: BTreeSet<(u64, Digest)>,
    /// Transactions handed to the engine and not yet seen in a block.
    queued: BTreeSet<[u8; 32]>,
    /// Kept to build the next epoch's engine.
    signer: Option<Arc<SigningKey>>,
    params: Params,
    dir: PathBuf,
}

impl core::fmt::Debug for BftDriver {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BftDriver")
            .field("id", &self.id)
            .field("epoch", &self.epoch)
            .field("round", &self.engine.round())
            .finish_non_exhaustive()
    }
}

impl BftDriver {
    /// Opens the epoch's safety log under `dir`, restores the engine from it,
    /// and returns the driver with whatever the replay produced (blocks for
    /// anchors committed but not yet built before the restart).
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] if the log cannot be opened or a safety record
    /// is corrupt; [`NodeError::Decode`] if the signer is not in the committee.
    pub fn open(
        setup: &BftSetup,
        dir: &Path,
        chain: &mut Chain,
        now_ms: u64,
    ) -> Result<(Self, Step)> {
        let (epoch, committee) = Self::committee_of(setup, chain)?;
        let mut step = Step::default();
        let driver = Self::boot(
            setup.signer.clone(),
            setup.params,
            dir.to_path_buf(),
            epoch,
            &committee,
            chain,
            now_ms,
            &mut step,
        )?;
        Ok((driver, step))
    }

    /// The epoch and committee to run: staking's, where the chain has it,
    /// else the genesis committee in `setup`.
    fn committee_of(setup: &BftSetup, chain: &Chain) -> Result<(u64, Arc<[VerifyingKey]>)> {
        match chain.state().committed_staking()? {
            Some(record) => {
                let keys = chain.state().validator_keys(&record.staking.active)?;
                Ok((record.staking.epoch, keys.into()))
            }
            None => Ok((setup.epoch, Arc::clone(&setup.committee))),
        }
    }

    /// Builds one epoch's engine, restores it from that epoch's safety log,
    /// and starts it. A signer outside the committee runs as an observer: it
    /// was not chosen this epoch, and it still has to follow the chain.
    #[allow(clippy::too_many_arguments)]
    fn boot(
        signer: Option<Arc<SigningKey>>,
        params: Params,
        dir: PathBuf,
        epoch: u64,
        committee: &Arc<[VerifyingKey]>,
        chain: &mut Chain,
        now_ms: u64,
        step: &mut Step,
    ) -> Result<Self> {
        let id = match &signer {
            None => None,
            Some(key) => {
                let mine = key.verifying_key();
                committee
                    .iter()
                    .position(|k| *k == mine)
                    .map(u16::try_from)
                    .transpose()
                    .map_err(|_| NodeError::Decode("committee index exceeds u16".to_string()))?
            }
        };
        let size = u16::try_from(committee.len())
            .map_err(|_| NodeError::Decode("committee exceeds u16".to_string()))?;
        let engine_params = Params { epoch, ..params };
        let engine = match (id, &signer) {
            (Some(id), Some(key)) => Validator::with_auth(
                id,
                Committee::new(size),
                engine_params,
                MlDsaAuthenticator::validator(Arc::clone(key), Arc::clone(committee)),
            ),
            _ => Validator::observer(
                Committee::new(size),
                engine_params,
                MlDsaAuthenticator::observer(Arc::clone(committee)),
            ),
        };
        let (store, recovered) = SafetyStore::open(&dir, epoch)?;
        let mut driver = Self {
            engine,
            id,
            epoch,
            store,
            logged: BTreeSet::new(),
            queued: BTreeSet::new(),
            signer,
            params,
            dir,
        };
        // Own proposals and votes first: they set the round, so replaying
        // certificates cannot make the engine sign a slot it already signed.
        for envelope in recovered.safety {
            match envelope.message {
                Message::Propose { vertex, signature } => {
                    driver.engine.restore_proposal(vertex, signature);
                }
                Message::Vote { digest, round, .. } => {
                    // The vote's author is who it was sent to.
                    driver.engine.restore_vote(round, envelope.to, digest);
                }
                Message::Cert(_) | Message::Fetch(_) => {}
            }
        }
        for c in recovered.certificates {
            driver.logged.insert((c.vertex.round, c.digest()));
            let from = c.vertex.author;
            let out = driver.engine.handle(now_ms, from, Message::Cert(c));
            driver.absorb(chain, now_ms, out, step)?;
        }
        let out = driver.engine.start(now_ms);
        driver.absorb(chain, now_ms, out, step)?;
        Ok(driver)
    }

    /// The committee epoch this driver runs.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// This node's validator id, or `None` for an observer.
    #[must_use]
    pub fn validator_id(&self) -> Option<ValidatorId> {
        self.id
    }

    /// Round most recently proposed (0 for an observer).
    #[must_use]
    pub fn round(&self) -> u64 {
        self.engine.round()
    }

    /// Round of the last committed anchor.
    #[must_use]
    pub fn last_committed_round(&self) -> u64 {
        self.engine.last_committed_round()
    }

    /// Queues `tx` for a future vertex, once. An observer queues nothing.
    pub fn submit(&mut self, tx: &Transaction) {
        if self.id.is_some() && self.queued.insert(tx.txid()) {
            self.engine.submit(tx.to_bytes());
        }
    }

    /// Whether `txid` is already queued.
    #[must_use]
    pub fn is_queued(&self, txid: &[u8; 32]) -> bool {
        self.queued.contains(txid)
    }

    /// Handles one gossip frame.
    ///
    /// # Errors
    ///
    /// A storage or chain fault. A frame that does not decode, names another
    /// epoch, or is addressed to someone else is not an error; it is ignored.
    pub fn on_frame(&mut self, chain: &mut Chain, now_ms: u64, bytes: &[u8]) -> Result<Step> {
        let mut step = Step::default();
        let Ok(envelope) = Envelope::decode(bytes) else {
            return Ok(step);
        };
        let addressed = envelope.to == BROADCAST || Some(envelope.to) == self.id;
        if envelope.epoch != self.epoch || !addressed || Some(envelope.from) == self.id {
            return Ok(step);
        }
        if let Message::Cert(c) = &envelope.message {
            self.log_certificate(c)?;
        }
        let out = self.engine.handle(now_ms, envelope.from, envelope.message);
        self.absorb(chain, now_ms, out, &mut step)?;
        Ok(step)
    }

    /// Periodic work: re-broadcasts and anchor timeouts.
    ///
    /// # Errors
    ///
    /// As [`BftDriver::on_frame`].
    pub fn on_tick(&mut self, chain: &mut Chain, now_ms: u64) -> Result<Step> {
        let mut step = Step::default();
        let out = self.engine.tick(now_ms);
        self.absorb(chain, now_ms, out, &mut step)?;
        Ok(step)
    }

    fn log_certificate(&mut self, c: &Certificate) -> Result<()> {
        if self.logged.insert((c.vertex.round, c.digest())) {
            self.store.record_certificate(c)?;
        }
        Ok(())
    }

    /// Persists what must be persisted, turns sends into frames, and builds
    /// blocks from what committed.
    fn absorb(
        &mut self,
        chain: &mut Chain,
        now_ms: u64,
        out: Output,
        step: &mut Step,
    ) -> Result<()> {
        for (dest, message) in out.sends {
            let to = match dest {
                Dest::All => BROADCAST,
                Dest::To(v) => v,
            };
            let envelope = Envelope {
                epoch: self.epoch,
                from: self.id.unwrap_or(BROADCAST),
                to,
                message,
            };
            match &envelope.message {
                Message::Propose { .. } | Message::Vote { .. } => {
                    self.store.record_own(&envelope)?;
                }
                Message::Cert(c) => {
                    let c = c.clone();
                    self.log_certificate(&c)?;
                }
                Message::Fetch(_) => {}
            }
            step.frames.push(envelope.encode());
        }
        step.equivocations.extend(out.equivocations);
        for sub_dag in out.sub_dags {
            let anchor = &sub_dag.anchor.vertex;
            if anchor.epoch != self.epoch || self.already_built(chain, anchor.epoch, anchor.round) {
                continue;
            }
            let anchor_time = anchor.timestamp_ms;
            let block = build_block(chain, &sub_dag)?;
            let included: Vec<[u8; 32]> =
                block.transactions.iter().map(Transaction::txid).collect();
            match chain.insert_block(block)? {
                InsertOutcome::Extended { tip } => {
                    step.blocks.push(tip);
                    step.anchor_times_ms.push(anchor_time);
                    for txid in &included {
                        self.queued.remove(txid);
                    }
                    step.included.extend(included);
                    if self.next_epoch(chain)?.is_some() {
                        // Everything the old engine committed after the
                        // boundary block belongs to a committee that no
                        // longer orders the chain.
                        return self.switch_epoch(chain, now_ms, step);
                    }
                }
                other => {
                    return Err(NodeError::Storage(format!(
                        "a locally built DAG-BFT block did not extend the tip: {other:?}"
                    )));
                }
            }
        }
        let horizon = self
            .engine
            .last_committed_round()
            .saturating_sub(maya_dag_bft::GC_DEPTH);
        self.logged = self.logged.split_off(&(horizon, [0; 32]));
        Ok(())
    }

    /// The staking epoch the chain has moved to, if it is not this driver's.
    fn next_epoch(&self, chain: &Chain) -> Result<Option<u64>> {
        Ok(chain
            .state()
            .committed_staking()?
            .map(|r| r.staking.epoch)
            .filter(|e| *e != self.epoch))
    }

    /// Replaces this epoch's engine with the next one's, whose committee is
    /// the staking module's new active set. Every node does this after the
    /// same block, so every node switches at the same point in the order.
    fn switch_epoch(&mut self, chain: &mut Chain, now_ms: u64, step: &mut Step) -> Result<()> {
        let record = chain
            .state()
            .committed_staking()?
            .ok_or_else(|| NodeError::Storage("staking record vanished".to_string()))?;
        let committee: Arc<[VerifyingKey]> =
            chain.state().validator_keys(&record.staking.active)?.into();
        let next = Self::boot(
            self.signer.clone(),
            self.params,
            self.dir.clone(),
            record.staking.epoch,
            &committee,
            chain,
            now_ms,
            step,
        )?;
        *self = next;
        Ok(())
    }

    /// Whether the anchor `(epoch, round)` is at or behind the tip's seal.
    fn already_built(&self, chain: &Chain, epoch: u64, round: u64) -> bool {
        if chain.height() == 0 {
            return false;
        }
        let Some(tip) = chain.get(&chain.tip()) else {
            return false;
        };
        let (tip_epoch, tip_round) = unseal(tip.header.nonce);
        (epoch, round) <= (tip_epoch, tip_round)
    }
}
