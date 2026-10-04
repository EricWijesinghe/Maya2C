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
    Certificate, Committee, Dest, Equivocation, Message, Output, Params, Validator, ValidatorId,
};

use super::attest::{ATTEST_FRAME, Attestation, Checkpoint, Collected, Collector};
use super::auth::MlDsaAuthenticator;
use super::builder::{build_block, unseal};
use super::remote::ValidatorKey;
use super::store::SafetyStore;
use super::wire::{BROADCAST, Envelope};
use crate::consensus::{BlockId, Chain, InsertOutcome};
use crate::core::{ChainTag, Transaction};
use crate::crypto::keys::VerifyingKey;
use crate::error::{NodeError, Result};

/// Everything a node needs to run one epoch of DAG-BFT.
#[derive(Clone)]
pub struct BftSetup {
    /// Committee epoch.
    pub epoch: u64,
    /// The committee's verifying keys, in validator-id order.
    pub committee: Arc<[VerifyingKey]>,
    /// This node's validator key, local or remote, if it is a validator.
    pub signer: Option<ValidatorKey>,
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
    /// Transactions this node proposed that were ordered but not executed —
    /// a nonce ahead of its predecessor, a conflict — and are no longer
    /// queued. The caller re-submits the ones still valid; without this a
    /// transfer whose nonce-1 sibling was ordered first by another validator
    /// was stranded for good (measured: 25 of 100 in `examples/bft_tps.rs`).
    pub dropped: Vec<Transaction>,
    /// Each built block's anchor proposal time, milliseconds: what the
    /// finality-latency metric measures from.
    pub anchor_times_ms: Vec<u64>,
    /// Wall time spent building and inserting this step's blocks — the
    /// execution half of the node's work, as opposed to the protocol half.
    pub build_time: std::time::Duration,
    /// Equivocations this node witnessed, for the staking module.
    pub equivocations: Vec<Equivocation>,
    /// Housekeeping that failed without touching safety — an old epoch's
    /// logs that could not be removed. The node reports them; they cost
    /// disk, never a vote.
    pub notices: Vec<String>,
}

/// An epoch's committee keys and, on a stake-weighted chain, their weights.
pub type StakedCommittee = (Arc<[VerifyingKey]>, Option<Arc<[u64]>>);

/// One epoch's committee: its keys, and its voting weights where the chain
/// is stake-weighted (ADR-040 part 2).
struct EpochCommittee {
    keys: Arc<[VerifyingKey]>,
    weights: Option<Arc<[u64]>>,
}

impl EpochCommittee {
    /// The committee the staking record names for its current epoch, with
    /// the weights frozen at that epoch's boundary, if the chain has them.
    fn of_epoch(chain: &Chain, record: &crate::state::staking::StakingRecord) -> Result<Self> {
        // Keys and weights from the same boundary snapshot, so a mid-epoch
        // tombstone cannot leave them different lengths or orders.
        let ids = chain
            .state()
            .committee_ids(record.staking.epoch)?
            .unwrap_or_else(|| record.staking.active.clone());
        let keys: Arc<[VerifyingKey]> = chain.state().validator_keys(&ids)?.into();
        let weights = chain.state().committee_weights(record.staking.epoch)?;
        if weights.as_ref().is_some_and(|w| w.len() != keys.len()) {
            return Err(NodeError::Storage(
                "committee weights do not match the committee".to_string(),
            ));
        }
        Ok(Self {
            keys,
            weights: weights.map(Into::into),
        })
    }

    /// The engine's committee: weighted where the chain is, else equal.
    fn engine(&self) -> Result<Committee> {
        match &self.weights {
            Some(w) => Committee::weighted(w.to_vec()),
            None => u16::try_from(self.keys.len()).ok().map(Committee::new),
        }
        .ok_or_else(|| NodeError::Decode("committee exceeds u16".to_string()))
    }
}

/// The `(epoch, round)` sealed in the chain's tip, or `None` at genesis.
fn tip_seal(chain: &Chain) -> Option<(u64, u64)> {
    if chain.height() == 0 {
        return None;
    }
    chain.get(&chain.tip()).map(|tip| unseal(tip.header.nonce))
}

/// One epoch's engine, its safety log, and the bookkeeping between them.
pub struct BftDriver {
    engine: Validator<MlDsaAuthenticator>,
    id: Option<ValidatorId>,
    epoch: u64,
    store: SafetyStore,
    /// Slots whose certificate is already in the log, so a re-broadcast is not
    /// appended every tick. Keyed by slot, not digest: hashing a megabyte
    /// vertex on every receipt to dedupe a log was measurable, and
    /// certification leaves one certificate per slot. Pruned with the horizon.
    logged: BTreeSet<(u64, ValidatorId)>,
    /// Transactions handed to the engine and not yet seen in a block.
    queued: BTreeSet<[u8; 32]>,
    /// Kept to build the next epoch's engine.
    signer: Option<ValidatorKey>,
    params: Params,
    dir: PathBuf,
    /// This epoch's committee, which attestations are checked against.
    committee: Arc<[VerifyingKey]>,
    /// Its voting weights on a stake-weighted chain (ADR-040 part 2).
    weights: Option<Arc<[u64]>>,
    /// Attestations gathered into the newest checkpoint (ADR-038).
    attestations: Collector,
    /// Set after catching up through a checkpoint: this node lacks the DAG
    /// history that says which vertices were already ordered, so a block it
    /// derived could differ from the network's. It votes and proposes, which
    /// is what restores fault tolerance, and imports attested blocks instead
    /// of building (ADR-038).
    follower: bool,
    /// The anchor round the engine resumed after when it became a follower.
    resumed_at: u64,
    /// The round of the last anchor this engine committed, built or not.
    last_anchor: Option<u64>,
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

    /// The committee and weights of the chain's current staking epoch, or
    /// `None` on a chain without staking (its genesis committee never
    /// changes). For a follower outside the engine, which must check each new
    /// checkpoint against the epoch it was signed in, not the one it started in.
    ///
    /// # Errors
    ///
    /// A storage failure, or keys and weights that do not match.
    pub fn staked_committee(chain: &Chain) -> Result<Option<StakedCommittee>> {
        chain
            .state()
            .committed_staking()?
            .map(|record| EpochCommittee::of_epoch(chain, &record).map(|c| (c.keys, c.weights)))
            .transpose()
    }

    /// The epoch and committee to run: staking's, where the chain has it,
    /// else the genesis committee in `setup`.
    fn committee_of(setup: &BftSetup, chain: &Chain) -> Result<(u64, EpochCommittee)> {
        match chain.state().committed_staking()? {
            Some(record) => {
                let committee = EpochCommittee::of_epoch(chain, &record)?;
                Ok((record.staking.epoch, committee))
            }
            None => Ok((
                setup.epoch,
                EpochCommittee {
                    keys: Arc::clone(&setup.committee),
                    weights: None,
                },
            )),
        }
    }

    /// Builds one epoch's engine, restores it from that epoch's safety log,
    /// and starts it. A signer outside the committee runs as an observer: it
    /// was not chosen this epoch, and it still has to follow the chain.
    #[allow(clippy::too_many_arguments)]
    fn boot(
        signer: Option<ValidatorKey>,
        params: Params,
        dir: PathBuf,
        epoch: u64,
        committee: &EpochCommittee,
        chain: &mut Chain,
        now_ms: u64,
        step: &mut Step,
    ) -> Result<Self> {
        let engine_committee = committee.engine()?;
        let weights = committee.weights.clone();
        let committee = &committee.keys;
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
        let engine_params = Params { epoch, ..params };
        let engine = match (id, &signer) {
            (Some(id), Some(key)) => Validator::with_auth(
                id,
                engine_committee,
                engine_params,
                MlDsaAuthenticator::validator(key.clone(), Arc::clone(committee)),
            ),
            _ => Validator::observer(
                engine_committee,
                engine_params,
                MlDsaAuthenticator::observer(Arc::clone(committee)),
            ),
        };
        let (store, recovered) = SafetyStore::open(&dir, epoch)?;
        // Here, at every boot and every epoch switch, so the logs on disk
        // never exceed this epoch's and the previous one's.
        let first_kept = epoch.saturating_sub(super::store::RETAINED_PAST_EPOCHS);
        if let Err(e) = super::store::prune_epochs_before(&dir, first_kept) {
            step.notices
                .push(format!("could not prune old epoch logs: {e}"));
        }
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
            committee: Arc::clone(committee),
            weights,
            attestations: Collector::default(),
            follower: false,
            resumed_at: 0,
            last_anchor: None,
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
            driver.logged.insert((c.vertex.round, c.vertex.author));
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
        if bytes.first() == Some(&ATTEST_FRAME) {
            self.on_attestation(chain, bytes, &mut step);
            return Ok(step);
        }
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

    /// Switches to following attested blocks after a catch-up (ADR-038):
    /// the engine resumes at the anchor round sealed in the chain's tip, and
    /// from here on this node votes and proposes but imports blocks rather
    /// than deriving them. A restart without catching up again leaves it.
    pub fn follow_attested(&mut self, chain: &Chain) {
        self.follower = true;
        if let Some((epoch, round)) = tip_seal(chain)
            && epoch == self.epoch
        {
            self.engine.resume_after(round);
            self.resumed_at = round;
        }
    }

    /// Whether a follower may build the block for the anchor at `round`
    /// itself again. Two things must hold. The anchor is more than
    /// `GC_DEPTH` rounds past where the engine resumed, so every vertex it
    /// could order is one this node received while running — the history
    /// it never saw lies below the horizon for it and for every peer alike,
    /// so its sub-DAG is the network's. And the chain's tip is the block of
    /// the anchor just before this one, so the new block has the parent the
    /// network's does. Until then, blocks keep coming from attested imports.
    fn may_build_again(&self, chain: &Chain, round: u64) -> bool {
        let past_window = round > self.resumed_at + maya_dag_bft::GC_DEPTH + 2;
        let at_previous = self
            .last_anchor
            .is_some_and(|prev| tip_seal(chain) == Some((self.epoch, prev)));
        past_window && at_previous
    }

    /// This epoch's committee: what attestations and checkpoints are
    /// checked against.
    #[must_use]
    pub fn committee(&self) -> Arc<[VerifyingKey]> {
        Arc::clone(&self.committee)
    }

    /// This epoch's voting weights on a stake-weighted chain; `None` where
    /// every member counts one. Catch-up checks checkpoints with these.
    #[must_use]
    pub fn weights(&self) -> Option<Arc<[u64]>> {
        self.weights.clone()
    }

    /// What `get_bft_status` reports for this driver.
    #[must_use]
    pub fn status(&self) -> crate::rpc::types::BftStatus {
        crate::rpc::types::BftStatus {
            epoch: self.epoch,
            round: self.engine.round(),
            committed_round: self.engine.last_committed_round(),
            validator: self.id,
            committee: u16::try_from(self.committee.len()).unwrap_or(u16::MAX),
            follower: self.follower,
        }
    }

    /// Whether this node follows attested blocks instead of building.
    #[must_use]
    pub fn is_follower(&self) -> bool {
        self.follower
    }

    /// The newest block a quorum of this epoch's committee attested: what a
    /// node that fell behind imports up to (ADR-038).
    #[must_use]
    pub fn checkpoint(&self) -> Option<&Checkpoint> {
        self.attestations.newest()
    }

    /// Signs and gossips this validator's attestation of the block it just
    /// built at the tip, and counts it. An observer attests nothing; a
    /// signature that failed costs a checkpoint, never safety.
    fn attest(&mut self, chain: &Chain, block: BlockId, step: &mut Step) {
        let (Some(id), Some(signer)) = (self.id, &self.signer) else {
            return;
        };
        let tag = ChainTag::from_genesis(chain.genesis());
        let height = chain.height();
        let Some(signature) = signer.sign_attestation(id, &tag, self.epoch, height, &block) else {
            return;
        };
        let attestation = Attestation {
            epoch: self.epoch,
            height,
            block,
            validator: id,
            signature,
        };
        step.frames.push(attestation.encode());
        self.collect(attestation, &tag, step);
    }

    /// An attestation frame from a peer: counted if it verifies against this
    /// epoch's committee, ignored otherwise, like any frame that does not
    /// decode or names another epoch.
    fn on_attestation(&mut self, chain: &Chain, bytes: &[u8], step: &mut Step) {
        let Ok(attestation) = Attestation::decode(bytes) else {
            return;
        };
        if attestation.epoch != self.epoch {
            return;
        }
        let tag = ChainTag::from_genesis(chain.genesis());
        self.collect(attestation, &tag, step);
    }

    fn collect(&mut self, attestation: Attestation, tag: &ChainTag, step: &mut Step) {
        // Counted, ignored, a new checkpoint, or a forged attestation refused
        // before it counted: only an equivocation is for the node to act on.
        if let Ok(Collected::Equivocation(pair)) =
            self.attestations
                .add(attestation, tag, &self.committee, self.weights.as_deref())
        {
            step.notices.push(format!(
                "validator {} attested two blocks at height {} — evidence held",
                pair.1.validator, pair.1.height
            ));
        }
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
        if self.logged.insert((c.vertex.round, c.vertex.author)) {
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
            let round = anchor.round;
            if self.follower {
                if !self.may_build_again(chain, round) {
                    // Blocks come from attested imports; see `follower`.
                    self.last_anchor = Some(round);
                    continue;
                }
                self.follower = false;
                step.notices.push(format!(
                    "caught up: building again from the anchor at round {round}"
                ));
            }
            self.last_anchor = Some(round);
            if anchor.epoch != self.epoch || self.already_built(chain, anchor.epoch, anchor.round) {
                continue;
            }
            let anchor_time = anchor.timestamp_ms;
            let started = std::time::Instant::now();
            let block = build_block(chain, &sub_dag)?;
            let included: Vec<[u8; 32]> =
                block.transactions.iter().map(Transaction::txid).collect();
            let kept: BTreeSet<[u8; 32]> = included.iter().copied().collect();
            for tx in sub_dag
                .certificates
                .iter()
                .filter(|c| Some(c.vertex.author) == self.id)
                .flat_map(|c| c.vertex.batch.iter())
                .filter_map(|bytes| Transaction::from_bytes(bytes).ok())
            {
                let id = tx.txid();
                if !kept.contains(&id) && self.queued.remove(&id) {
                    step.dropped.push(tx);
                }
            }
            let inserted = chain.insert_block(block)?;
            step.build_time += started.elapsed();
            match inserted {
                InsertOutcome::Extended { tip } => {
                    step.blocks.push(tip);
                    self.attest(chain, tip, step);
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
        self.logged = self.logged.split_off(&(horizon, 0));
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
        let committee = EpochCommittee::of_epoch(chain, &record)?;
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
