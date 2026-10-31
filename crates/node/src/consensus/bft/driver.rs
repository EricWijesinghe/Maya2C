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

use std::collections::{BTreeMap, BTreeSet};
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
use crate::crypto::validator_keys::VerifyingKey;
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

/// A transaction this node handed to the engine and has not seen included.
pub(super) struct Queued {
    /// blake3 of the encoding the engine carries, so a committed payload is
    /// matched without decoding it.
    raw: [u8; 32],
    tx: Transaction,
}

/// Removes the queued transactions this node proposed that the sub-DAG carried
/// (`own`, blake3 of each payload) but the block did not include (`kept`), and
/// returns them for re-submission. Matched by hash, so a payload past the
/// decode budget is never decoded, yet its queued copy is still released.
pub(super) fn release_unincluded(
    queued: &mut BTreeMap<[u8; 32], Queued>,
    own: &BTreeSet<[u8; 32]>,
    kept: &BTreeSet<[u8; 32]>,
) -> Vec<Transaction> {
    let ids: Vec<[u8; 32]> = queued
        .iter()
        .filter(|(id, q)| own.contains(&q.raw) && !kept.contains(*id))
        .map(|(id, _)| *id)
        .collect();
    ids.iter()
        .filter_map(|id| queued.remove(id))
        .map(|q| q.tx)
        .collect()
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
    /// Transactions this node proposed that were ordered but not included — a
    /// nonce ahead of its predecessor, a conflict, or past the block cap
    /// (ADR-072) — and are no longer queued. The caller re-submits the ones
    /// still valid; without this a transfer whose nonce-1 sibling was ordered
    /// first by another validator was stranded for good (measured: 25 of 100
    /// in `examples/bft_tps.rs`).
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

/// How far above this node's tip an attestation may be and still count. A
/// collector forgets partial sets far below the newest height it has seen,
/// so one validly signed attestation at an absurd height from a single
/// Byzantine member would otherwise wipe every honest partial set.
const ATTEST_AHEAD: u64 = 64;

/// How long a builder must stay overtaken *and stuck* before it turns
/// follower. Behind is not enough: a validator restarted beside others
/// commits its way back over some seconds while the follow loop imports
/// ahead of it, and flipping it then cost a voter and stalled the chain
/// (attacknet soak, 2026-10-05). Only an engine that commits nothing for
/// this long while blocks arrive is rescued.
const OVERTAKEN_GRACE_MS: u64 = 20_000;

/// The previous epoch's committee and its collector, kept after a switch.
struct Closing {
    epoch: u64,
    /// The epoch's last block: nothing above it was ever that committee's.
    last_height: u64,
    keys: Arc<[VerifyingKey]>,
    weights: Option<Arc<[u64]>>,
    attestations: Collector,
}

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
    /// Transactions handed to the engine and not yet seen in a block, by id.
    queued: BTreeMap<[u8; 32], Queued>,
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
    /// The epoch just ended, still collecting: attestations of its last
    /// blocks, the boundary block among them, arrive after every node has
    /// switched, and its final checkpoint is what carries a node that slept
    /// through the boundary into the new committee.
    closing: Option<Closing>,
    /// The round of the last anchor this engine committed, built or not.
    last_anchor: Option<u64>,
    /// When the tip was first seen built for an anchor this engine had not
    /// committed, and the engine's committed round then; reset whenever the
    /// engine commits again (see `overtaken`).
    overtaken_since: Option<(u64, u64)>,
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
                MlDsaAuthenticator::validator(
                    key.clone(),
                    Arc::clone(committee),
                    ChainTag::from_genesis(chain.genesis()),
                ),
            ),
            _ => Validator::observer(
                engine_committee,
                engine_params,
                MlDsaAuthenticator::observer(
                    Arc::clone(committee),
                    ChainTag::from_genesis(chain.genesis()),
                ),
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
            queued: BTreeMap::new(),
            signer,
            params,
            dir,
            committee: Arc::clone(committee),
            weights,
            attestations: Collector::default(),
            follower: false,
            resumed_at: 0,
            // The anchor the tip was built for, so a block that later
            // arrives from elsewhere is recognised as one this engine never
            // committed (see `overtaken`).
            last_anchor: tip_seal(chain)
                .filter(|(e, _)| *e == epoch)
                .map(|(_, round)| round),
            closing: None,
            overtaken_since: None,
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
        // Streamed, one certificate at a time: an epoch's log is never held
        // whole (ADR-052 memory bound).
        let replay_dir = driver.dir.clone();
        let skipped = super::store::for_each_certificate(&replay_dir, epoch, |c| {
            driver.logged.insert((c.vertex.round, c.vertex.author));
            let from = c.vertex.author;
            let out = driver.engine.handle(now_ms, from, Message::Cert(c));
            driver.absorb(chain, now_ms, out, step)
        })?;
        if skipped > 0 {
            step.notices.push(format!(
                "skipped {skipped} undecodable certificate record(s) in epoch {epoch}"
            ));
        }
        if recovered.torn_bytes > 0 {
            step.notices.push(format!(
                "cut {} torn byte(s) from the epoch {epoch} logs",
                recovered.torn_bytes
            ));
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
        let txid = tx.txid();
        if self.id.is_none() || self.queued.contains_key(&txid) {
            return;
        }
        let bytes = tx.to_bytes();
        let raw = *blake3::hash(&bytes).as_bytes();
        self.queued.insert(
            txid,
            Queued {
                raw,
                tx: tx.clone(),
            },
        );
        self.engine.submit(bytes);
    }

    /// Whether `txid` is already queued.
    #[must_use]
    pub fn is_queued(&self, txid: &[u8; 32]) -> bool {
        self.queued.contains_key(txid)
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
        // Not logged here: a frame's certificate is unverified until the
        // engine has checked its signatures and parents. `absorb` logs what
        // the engine inserted, so a forged certificate never takes a slot
        // in certs.log ahead of the real one.
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

    /// Whether the chain's tip was built for an anchor this engine has not
    /// committed: a block imported from a peer's checkpoint, so this node is
    /// behind, whatever made it so. A builder that missed rounds would
    /// otherwise wait for a sub-DAG it can no longer complete; following
    /// attested blocks brings it back (attacknet soak, 2026-10-05).
    fn overtaken(&self, chain: &Chain) -> bool {
        let Some((epoch, round)) = tip_seal(chain) else {
            return false;
        };
        // More than one anchor (anchors are even rounds) ahead of the engine.
        epoch == self.epoch && self.last_anchor.is_none_or(|last| round > last + 2)
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

    /// The newest checkpoint of the epoch that just ended, completed by
    /// attestations that arrived after the switch.
    #[must_use]
    pub fn closing_checkpoint(&self) -> Option<&Checkpoint> {
        self.closing.as_ref().and_then(|c| c.attestations.newest())
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
        let tag = ChainTag::from_genesis(chain.genesis());
        if attestation.epoch == self.epoch {
            if attestation.height <= chain.height().saturating_add(ATTEST_AHEAD) {
                self.collect(attestation, &tag, step);
            }
        } else if let Some(closing) = self
            .closing
            .as_mut()
            .filter(|c| c.epoch == attestation.epoch && attestation.height <= c.last_height)
        {
            // Late attestations of the last epoch's final blocks, checked
            // against that epoch's committee. A forged one is refused before
            // it counts, which is all an error here means; an equivocation is
            // reported like one in the live epoch.
            if let Ok(Collected::Equivocation(pair)) = closing.attestations.add(
                attestation,
                &tag,
                &closing.keys,
                closing.weights.as_deref(),
            ) {
                step.notices.push(format!(
                    "validator {} attested two blocks at height {} of epoch {} — evidence held",
                    pair.1.validator, pair.1.height, pair.1.epoch
                ));
            }
        }
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
        // A builder switches epochs on the boundary block it builds. A block
        // imported instead (a follower's, or one a former follower's
        // follow_loop brought in) is noticed here, or the node would stay in
        // an epoch whose committee no longer orders the chain.
        if self.next_epoch(chain)?.is_some() {
            let was_following = self.follower;
            self.switch_epoch(chain, now_ms, &mut step)?;
            if was_following {
                self.follow_attested(chain);
            }
            step.notices.push(format!(
                "followed attested blocks into epoch {}",
                self.epoch
            ));
            return Ok(step);
        }
        let committed = self.engine.last_committed_round();
        if self.follower || !self.overtaken(chain) {
            self.overtaken_since = None;
        } else if self.overtaken_since.is_none_or(|(_, at)| committed > at) {
            // Newly overtaken, or the engine is still committing: wait.
            self.overtaken_since = Some((now_ms, committed));
        } else if self
            .overtaken_since
            .is_some_and(|(since, _)| now_ms.saturating_sub(since) >= OVERTAKEN_GRACE_MS)
        {
            self.overtaken_since = None;
            self.follow_attested(chain);
            step.notices.push(
                "blocks arrived that this node did not build: following attested blocks"
                    .to_string(),
            );
        }
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
        // Before any send: an own certificate is on disk before a peer can
        // build on it.
        for c in &out.inserted {
            self.log_certificate(c)?;
        }
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
                Message::Cert(_) | Message::Fetch(_) => {}
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
            // Matched by hash, not decoded: a payload past the decode budget
            // is never decoded, yet its queued copy must still be released.
            let own: BTreeSet<[u8; 32]> = sub_dag
                .certificates
                .iter()
                .filter(|c| Some(c.vertex.author) == self.id)
                .flat_map(|c| c.vertex.batch.iter())
                .map(|bytes| *blake3::hash(bytes).as_bytes())
                .collect();
            step.dropped
                .extend(release_unincluded(&mut self.queued, &own, &kept));
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
        let closing = Closing {
            epoch: self.epoch,
            last_height: chain.height(),
            keys: Arc::clone(&self.committee),
            weights: self.weights.clone(),
            attestations: std::mem::take(&mut self.attestations),
        };
        *self = next;
        self.closing = Some(closing);
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

#[cfg(test)]
mod release_tests {
    use super::*;
    use crate::core::transaction::TxOutput;
    use primitive_types::U256;

    fn tx(nonce: u64) -> Transaction {
        let output = TxOutput {
            amount: U256::from(1_u64),
            recipient: [7; 32],
        };
        Transaction::new(vec![], vec![output], nonce)
    }

    fn hash_of(tx: &Transaction) -> [u8; 32] {
        *blake3::hash(&tx.to_bytes()).as_bytes()
    }

    fn queue(txs: &[Transaction]) -> BTreeMap<[u8; 32], Queued> {
        txs.iter()
            .map(|t| {
                (
                    t.txid(),
                    Queued {
                        raw: hash_of(t),
                        tx: t.clone(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn an_own_payload_not_included_is_released_by_hash_and_the_rest_stay_queued() {
        let (a, b, c) = (tx(1), tx(2), tx(3));
        let mut queued = queue(&[a.clone(), b.clone(), c.clone()]);
        // The sub-DAG carried a and b from this node; c was not in it.
        let own: BTreeSet<[u8; 32]> = [hash_of(&a), hash_of(&b)].into_iter().collect();
        // The block included a, so only b is released.
        let kept: BTreeSet<[u8; 32]> = [a.txid()].into_iter().collect();

        let released = release_unincluded(&mut queued, &own, &kept);

        assert_eq!(released, vec![b.clone()]);
        assert!(
            queued.contains_key(&a.txid()),
            "included stays until the caller removes it"
        );
        assert!(!queued.contains_key(&b.txid()));
        assert!(queued.contains_key(&c.txid()));
    }

    #[test]
    fn a_payload_past_the_decode_cut_is_released_without_being_decoded() {
        // Only the hash of an own payload is known here; no decode happens.
        let past_cut = tx(9);
        let mut queued = queue(&[past_cut.clone()]);
        let own: BTreeSet<[u8; 32]> = [hash_of(&past_cut)].into_iter().collect();
        let released = release_unincluded(&mut queued, &own, &BTreeSet::new());
        assert_eq!(released, vec![past_cut]);
        assert!(queued.is_empty());
    }
}
