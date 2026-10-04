//! Block attestations and checkpoints (ADR-038, mainnet gate 8).
//!
//! On a DAG-BFT chain a block is a function of certificates, and a node never
//! imports a block it did not derive — so a validator that missed the
//! certificates (down longer than the engine's 50-round window) could never
//! derive the blocks, and never rejoined. Here every validator signs each
//! block it built; `2f + 1` such signatures from the epoch's committee make
//! the block a **checkpoint**. A lagging node imports blocks up to a
//! checkpoint, re-executing each (invariant 24 refuses a state root its own
//! execution does not reproduce), so the peer that serves them is trusted for
//! nothing: the quorum names the chain, re-execution checks the state.
//!
//! Engine-free and transport-free, like the rest of `consensus::bft`'s pure
//! parts: the driver signs and gossips, the node fetches and imports.

use std::collections::BTreeMap;

use maya_dag_bft::Committee;

use crate::core::ChainTag;
use crate::crypto::keys::{SIGNATURE_LENGTH, SigningKey, VerifyingKey};
use crate::error::{NodeError, Result};

/// Domain of the bytes an attestation signs, distinct from every vertex,
/// vote and transaction domain so no signature can be replayed as another.
/// Defined by the remote signer, which must sign exactly these bytes.
pub use maya_signer::service::ATTEST_DOMAIN;

/// First byte of an attestation frame on the BFT topic. Engine frames start
/// with `wire::WIRE_VERSION`; this byte differs, so the two never parse as
/// each other.
pub const ATTEST_FRAME: u8 = 0xA7;

/// Encoded length of an attestation frame.
const FRAME_LEN: usize = 1 + 8 + 8 + 32 + 2 + SIGNATURE_LENGTH;

/// How many recent heights the collector keeps partial attestation sets
/// for. A block's attestations arrive within a few rounds of it; older
/// partial sets will not complete and only cost memory.
const PENDING_HEIGHTS: u64 = 64;

/// What an attestation commits to: `blake3(chain ‖ epoch ‖ height ‖ block)`,
/// keyed under [`ATTEST_DOMAIN`]. A digest, so the remote signer handles 32
/// bytes per request as it does for vertices and votes (ADR-033).
#[must_use]
pub fn attestation_digest(chain: &ChainTag, epoch: u64, height: u64, block: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("maya2c block attestation digest v1");
    h.update(chain.as_bytes());
    h.update(&epoch.to_le_bytes());
    h.update(&height.to_le_bytes());
    h.update(block);
    *h.finalize().as_bytes()
}

/// The bytes a validator signs to attest that block `block` is height
/// `height` of the chain `chain`, in staking epoch `epoch`:
/// [`ATTEST_DOMAIN`] then [`attestation_digest`].
#[must_use]
pub fn attestation_bytes(chain: &ChainTag, epoch: u64, height: u64, block: &[u8; 32]) -> Vec<u8> {
    [
        ATTEST_DOMAIN,
        attestation_digest(chain, epoch, height, block).as_slice(),
    ]
    .concat()
}

/// One validator's signature on one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
    /// Staking epoch whose committee the validator belongs to.
    pub epoch: u64,
    /// Height of the block.
    pub height: u64,
    /// The block's id.
    pub block: [u8; 32],
    /// The validator's index in that epoch's committee.
    pub validator: u16,
    /// ML-DSA-65 over [`attestation_bytes`].
    pub signature: Vec<u8>,
}

impl Attestation {
    /// Signs block `block` at `height` as committee member `validator`.
    ///
    /// # Errors
    ///
    /// The signer failed (ML-DSA's rejection loop may report this).
    pub fn sign(
        key: &SigningKey,
        validator: u16,
        chain: &ChainTag,
        epoch: u64,
        height: u64,
        block: [u8; 32],
    ) -> Result<Self> {
        let signature = key
            .sign(&attestation_bytes(chain, epoch, height, &block))?
            .to_vec();
        Ok(Self {
            epoch,
            height,
            block,
            validator,
            signature,
        })
    }

    /// Checks the signature against `committee[validator]`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a validator outside the committee or a
    /// signature of the wrong length; [`NodeError::SignatureVerification`]
    /// if it does not verify.
    pub fn verify(&self, chain: &ChainTag, committee: &[VerifyingKey]) -> Result<()> {
        let key = committee.get(usize::from(self.validator)).ok_or_else(|| {
            NodeError::Decode(format!(
                "attestation from validator {} outside the committee",
                self.validator
            ))
        })?;
        let signature: &[u8; SIGNATURE_LENGTH] =
            self.signature.as_slice().try_into().map_err(|_| {
                NodeError::Decode("attestation signature has the wrong length".to_string())
            })?;
        key.verify(
            &attestation_bytes(chain, self.epoch, self.height, &self.block),
            signature,
        )
    }

    /// The frame gossiped on the BFT topic.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FRAME_LEN);
        out.push(ATTEST_FRAME);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.block);
        out.extend_from_slice(&self.validator.to_le_bytes());
        out.extend_from_slice(&self.signature);
        out
    }

    /// Parses a frame written by [`Attestation::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for any other length or first byte.
    pub fn decode(frame: &[u8]) -> Result<Self> {
        if frame.len() != FRAME_LEN || frame[0] != ATTEST_FRAME {
            return Err(NodeError::Decode("not an attestation frame".to_string()));
        }
        let u64_at = |at: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&frame[at..at + 8]);
            u64::from_le_bytes(b)
        };
        let mut block = [0u8; 32];
        block.copy_from_slice(&frame[17..49]);
        Ok(Self {
            epoch: u64_at(1),
            height: u64_at(9),
            block,
            validator: u16::from_le_bytes([frame[49], frame[50]]),
            signature: frame[51..].to_vec(),
        })
    }
}

/// A block with a quorum of its epoch committee's attestations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    /// Staking epoch of the attesting committee.
    pub epoch: u64,
    /// Height of the block.
    pub height: u64,
    /// The block's id.
    pub block: [u8; 32],
    /// Signatures by committee index; ordered, so the encoding is canonical.
    pub signatures: BTreeMap<u16, Vec<u8>>,
}

impl Checkpoint {
    /// Checks that a quorum of distinct `committee` members signed it, the
    /// quorum counted in `weights` (one per member, committee order) on a
    /// stake-weighted chain (ADR-040 part 2), or in heads where `weights` is
    /// `None`. There is no default: a caller that forgets the weights would
    /// count cheap seats as equals. A catching-up node imports blocks on this
    /// quorum alone, so on a weighted chain it must be stake, or cheap
    /// seats could attest a chain of their own making.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if fewer than a quorum signed or the weights do
    /// not match the committee, or as [`Attestation::verify`] for any one
    /// signature.
    pub fn verify(
        &self,
        chain: &ChainTag,
        committee: &[VerifyingKey],
        weights: Option<&[u64]>,
    ) -> Result<()> {
        if !has_quorum(self.signatures.keys().copied(), committee.len(), weights)? {
            return Err(NodeError::Decode(format!(
                "checkpoint at height {} is signed by {} member(s), short of a quorum",
                self.height,
                self.signatures.len()
            )));
        }
        for (&validator, signature) in &self.signatures {
            Attestation {
                epoch: self.epoch,
                height: self.height,
                block: self.block,
                validator,
                signature: signature.clone(),
            }
            .verify(chain, committee)?;
        }
        Ok(())
    }
}

/// Whether `signers` form a quorum of a committee of `size`: n − f heads
/// (ADR-039), or W − f stake when `weights` is given (ADR-040 part 2).
fn has_quorum(
    signers: impl IntoIterator<Item = u16>,
    size: usize,
    weights: Option<&[u64]>,
) -> Result<bool> {
    let committee = match weights {
        Some(w) if w.len() == size => Committee::weighted(w.to_vec()),
        Some(_) => {
            return Err(NodeError::Decode(
                "committee weights do not match the committee".to_string(),
            ));
        }
        None => u16::try_from(size).ok().map(Committee::new),
    }
    .ok_or_else(|| NodeError::Decode("committee larger than u16".to_string()))?;
    Ok(committee.is_quorum(signers))
}

/// How many epochs' final checkpoints a node keeps serving. A validator
/// down for longer catches up from a snapshot instead.
pub const KEPT_EPOCHS: usize = 8;

/// The newest checkpoint of each recent epoch: what `get_checkpoint` serves.
///
/// A node that was down across an epoch boundary trusts only its own
/// epoch's committee, so it cannot check a checkpoint the next committee
/// signed. It imports up to its own epoch's final checkpoint first; the
/// boundary block in that span writes the next committee into its state,
/// and the next epoch's checkpoint then checks out. One epoch at a time.
#[derive(Clone, Debug, Default)]
pub struct CheckpointBook {
    by_epoch: BTreeMap<u64, Checkpoint>,
}

impl CheckpointBook {
    /// Keeps `checkpoint` if it is newer than what its epoch holds, and
    /// forgets epochs beyond [`KEPT_EPOCHS`].
    pub fn offer(&mut self, checkpoint: &Checkpoint) {
        let newer = self
            .by_epoch
            .get(&checkpoint.epoch)
            .is_none_or(|held| held.height < checkpoint.height);
        if newer {
            self.by_epoch.insert(checkpoint.epoch, checkpoint.clone());
        }
        while self.by_epoch.len() > KEPT_EPOCHS {
            self.by_epoch.pop_first();
        }
    }

    /// The newest checkpoint of the newest epoch.
    #[must_use]
    pub fn newest(&self) -> Option<&Checkpoint> {
        self.by_epoch.last_key_value().map(|(_, c)| c)
    }

    /// The newest checkpoint held for `epoch`.
    #[must_use]
    pub fn of_epoch(&self, epoch: u64) -> Option<&Checkpoint> {
        self.by_epoch.get(&epoch)
    }
}

/// What a collector made of one attestation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Collected {
    /// Counted; no new checkpoint yet.
    Counted,
    /// Already held, or for a height at or below the newest checkpoint.
    Ignored,
    /// It completed a quorum: the newest checkpoint.
    Checkpoint(Checkpoint),
    /// The validator attested a different block at a height it had already
    /// attested: evidence of a broken or malicious node, kept for staking.
    Equivocation(Box<(Attestation, Attestation)>),
}

/// Signatures on one block by committee index.
type Signatures = BTreeMap<u16, Vec<u8>>;

/// Gathers attestations into checkpoints and keeps the newest.
#[derive(Debug, Default)]
pub struct Collector {
    /// Partial sets by `(epoch, height, block)`.
    pending: BTreeMap<(u64, u64, [u8; 32]), Signatures>,
    /// The first block each validator attested at each pending height.
    seen: BTreeMap<(u64, u64, u16), [u8; 32]>,
    newest: Option<Checkpoint>,
}

impl Collector {
    /// The newest checkpoint, if any block has reached a quorum.
    #[must_use]
    pub fn newest(&self) -> Option<&Checkpoint> {
        self.newest.as_ref()
    }

    /// Adds a verified-on-entry attestation from `committee` (the epoch's),
    /// the quorum counted in `weights` on a stake-weighted chain, else heads.
    ///
    /// # Errors
    ///
    /// As [`Attestation::verify`]: a bad attestation is refused, never
    /// counted.
    pub fn add(
        &mut self,
        attestation: Attestation,
        chain: &ChainTag,
        committee: &[VerifyingKey],
        weights: Option<&[u64]>,
    ) -> Result<Collected> {
        if self
            .newest
            .as_ref()
            .is_some_and(|c| (attestation.epoch, attestation.height) <= (c.epoch, c.height))
        {
            return Ok(Collected::Ignored);
        }
        attestation.verify(chain, committee)?;
        let slot = (attestation.epoch, attestation.height, attestation.validator);
        if let Some(first) = self.seen.get(&slot) {
            if *first == attestation.block {
                return Ok(Collected::Ignored);
            }
            let earlier = Attestation {
                block: *first,
                signature: self.pending[&(slot.0, slot.1, *first)][&slot.2].clone(),
                ..attestation.clone()
            };
            return Ok(Collected::Equivocation(Box::new((earlier, attestation))));
        }
        self.forget_stale(attestation.epoch, attestation.height);
        self.seen.insert(slot, attestation.block);
        let key = (attestation.epoch, attestation.height, attestation.block);
        let set = self.pending.entry(key).or_default();
        set.insert(attestation.validator, attestation.signature);
        if !has_quorum(set.keys().copied(), committee.len(), weights)? {
            return Ok(Collected::Counted);
        }
        let checkpoint = Checkpoint {
            epoch: key.0,
            height: key.1,
            block: key.2,
            signatures: set.clone(),
        };
        self.newest = Some(checkpoint.clone());
        self.forget_through(key.0, key.1);
        Ok(Collected::Checkpoint(checkpoint))
    }

    /// Drops partial sets at or below the new checkpoint: they can no
    /// longer make a newer one.
    fn forget_through(&mut self, epoch: u64, height: u64) {
        self.pending
            .retain(|&(e, h, _), _| (e, h) > (epoch, height));
        self.seen.retain(|&(e, h, _), _| (e, h) > (epoch, height));
    }

    /// Bounds memory when no quorum forms: partial sets more than
    /// [`PENDING_HEIGHTS`] below the newest height seen will never complete.
    fn forget_stale(&mut self, epoch: u64, height: u64) {
        let floor = height.saturating_sub(PENDING_HEIGHTS);
        let fresh = |e: u64, h: u64| e > epoch || (e == epoch && h >= floor);
        self.pending.retain(|&(e, h, _), _| fresh(e, h));
        self.seen.retain(|&(e, h, _), _| fresh(e, h));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::crypto::keys::signing_key_from_seed;

    const CHAIN: ChainTag = ChainTag::from_genesis([7; 32]);

    fn keys(n: u8) -> (Vec<SigningKey>, Vec<VerifyingKey>) {
        let signing: Vec<SigningKey> = (1..=n)
            .map(|i| signing_key_from_seed(&[i; 32]).unwrap())
            .collect();
        let verifying = signing.iter().map(SigningKey::verifying_key).collect();
        (signing, verifying)
    }

    fn attest(keys: &[SigningKey], v: u16, height: u64, block: u8) -> Attestation {
        Attestation::sign(&keys[usize::from(v)], v, &CHAIN, 0, height, [block; 32]).unwrap()
    }

    #[test]
    fn an_attestation_round_trips_and_verifies_only_for_its_chain_and_signer() {
        let (signing, committee) = keys(4);
        let a = attest(&signing, 2, 10, 9);
        let decoded = Attestation::decode(&a.encode()).unwrap();
        assert_eq!(decoded, a);
        decoded.verify(&CHAIN, &committee).unwrap();
        assert!(
            decoded
                .verify(&ChainTag::from_genesis([8; 32]), &committee)
                .is_err()
        );
        let claimed_by_other = Attestation {
            validator: 1,
            ..decoded.clone()
        };
        assert!(claimed_by_other.verify(&CHAIN, &committee).is_err());
        let outside = Attestation {
            validator: 4,
            ..decoded
        };
        assert!(outside.verify(&CHAIN, &committee).is_err());
    }

    #[test]
    fn a_frame_of_another_shape_is_refused() {
        let (signing, _) = keys(1);
        let frame = attest(&signing, 0, 1, 1).encode();
        assert!(Attestation::decode(&frame[..frame.len() - 1]).is_err());
        let mut engine_frame = frame.clone();
        engine_frame[0] = crate::consensus::bft::wire::WIRE_VERSION;
        assert!(Attestation::decode(&engine_frame).is_err());
    }

    #[test]
    fn three_of_four_make_a_checkpoint_that_verifies() {
        let (signing, committee) = keys(4);
        let mut collector = Collector::default();
        for v in 0..2 {
            assert_eq!(
                collector
                    .add(attest(&signing, v, 5, 1), &CHAIN, &committee, None)
                    .unwrap(),
                Collected::Counted
            );
        }
        let Collected::Checkpoint(c) = collector
            .add(attest(&signing, 3, 5, 1), &CHAIN, &committee, None)
            .unwrap()
        else {
            panic!("the third attestation completes a quorum of four");
        };
        assert_eq!((c.height, c.block, c.signatures.len()), (5, [1; 32], 3));
        c.verify(&CHAIN, &committee, None).unwrap();
        assert_eq!(collector.newest(), Some(&c));
        // Late and repeated attestations change nothing.
        assert_eq!(
            collector
                .add(attest(&signing, 2, 5, 1), &CHAIN, &committee, None)
                .unwrap(),
            Collected::Ignored
        );
    }

    #[test]
    fn a_checkpoint_short_of_quorum_or_with_a_forged_signature_is_refused() {
        let (signing, committee) = keys(4);
        let mut c = Checkpoint {
            epoch: 0,
            height: 3,
            block: [2; 32],
            signatures: BTreeMap::new(),
        };
        for v in 0..2u16 {
            c.signatures.insert(v, attest(&signing, v, 3, 2).signature);
        }
        assert!(
            c.verify(&CHAIN, &committee, None).is_err(),
            "two of four is not a quorum"
        );
        c.signatures.insert(2, attest(&signing, 2, 3, 99).signature);
        assert!(
            c.verify(&CHAIN, &committee, None).is_err(),
            "a signature on another block"
        );
        c.signatures.insert(2, attest(&signing, 2, 3, 2).signature);
        c.verify(&CHAIN, &committee, None).unwrap();
    }

    #[test]
    fn two_blocks_at_one_height_from_one_validator_is_equivocation() {
        let (signing, committee) = keys(4);
        let mut collector = Collector::default();
        collector
            .add(attest(&signing, 1, 8, 1), &CHAIN, &committee, None)
            .unwrap();
        let Collected::Equivocation(pair) = collector
            .add(attest(&signing, 1, 8, 2), &CHAIN, &committee, None)
            .unwrap()
        else {
            panic!("a second block at the same height must be evidence");
        };
        assert_eq!((pair.0.block, pair.1.block), ([1; 32], [2; 32]));
        pair.0.verify(&CHAIN, &committee).unwrap();
        pair.1.verify(&CHAIN, &committee).unwrap();
    }

    #[test]
    fn partial_sets_far_behind_the_newest_height_are_forgotten() {
        let (signing, committee) = keys(4);
        let mut collector = Collector::default();
        collector
            .add(attest(&signing, 0, 1, 1), &CHAIN, &committee, None)
            .unwrap();
        collector
            .add(
                attest(&signing, 0, 1 + PENDING_HEIGHTS + 1, 1),
                &CHAIN,
                &committee,
                None,
            )
            .unwrap();
        assert_eq!(
            collector.pending.len(),
            1,
            "height 1 can no longer complete"
        );
    }

    #[test]
    fn a_forged_attestation_is_never_counted() {
        let (signing, committee) = keys(4);
        let mut collector = Collector::default();
        let mut forged = attest(&signing, 0, 4, 1);
        forged.signature[0] ^= 1;
        assert!(collector.add(forged, &CHAIN, &committee, None).is_err());
        for v in 1..3 {
            collector
                .add(attest(&signing, v, 4, 1), &CHAIN, &committee, None)
                .unwrap();
        }
        assert!(
            collector.newest().is_none(),
            "the forged one must not have counted"
        );
    }

    #[test]
    fn cheap_seats_make_a_head_quorum_but_never_a_stake_quorum() {
        // ADR-040 part 2: three seats bought at 1,000 each against one
        // validator of 10,000. Three of four heads are a quorum; 3,000 of
        // 13,000 stake is not, so a catching-up node on a weighted chain
        // refuses the checkpoint the cheap seats sign.
        let (signing, committee) = keys(4);
        let weights = [1_000, 1_000, 1_000, 10_000];
        let mut heads = Collector::default();
        let mut stake = Collector::default();
        let mut made = None;
        for v in 0..3 {
            let a = attest(&signing, v, 5, 1);
            if let Collected::Checkpoint(c) =
                heads.add(a.clone(), &CHAIN, &committee, None).unwrap()
            {
                made = Some(c);
            }
            assert!(matches!(
                stake.add(a, &CHAIN, &committee, Some(&weights)).unwrap(),
                Collected::Counted
            ));
        }
        let c = made.expect("three of four heads are a quorum");
        c.verify(&CHAIN, &committee, None).unwrap();
        assert!(c.verify(&CHAIN, &committee, Some(&weights)).is_err());
        assert!(stake.newest().is_none());
        // The heavy validator completes the stake quorum.
        let Collected::Checkpoint(w) = stake
            .add(
                attest(&signing, 3, 5, 1),
                &CHAIN,
                &committee,
                Some(&weights),
            )
            .unwrap()
        else {
            panic!("the stake majority completes a quorum");
        };
        w.verify(&CHAIN, &committee, Some(&weights)).unwrap();
        // Weights of the wrong length are an error, never a silent head count.
        assert!(w.verify(&CHAIN, &committee, Some(&weights[..3])).is_err());
    }

    #[test]
    fn the_book_keeps_each_epochs_newest_checkpoint_and_forgets_old_epochs() {
        let (signing, _) = keys(1);
        let at = |epoch: u64, height: u64| Checkpoint {
            epoch,
            height,
            block: [1; 32],
            signatures: BTreeMap::from([(0, attest(&signing, 0, height, 1).signature)]),
        };
        let mut book = CheckpointBook::default();
        book.offer(&at(0, 10));
        book.offer(&at(0, 8)); // older: ignored
        book.offer(&at(1, 12));
        assert_eq!(book.of_epoch(0).map(|c| c.height), Some(10));
        assert_eq!(book.newest().map(|c| (c.epoch, c.height)), Some((1, 12)));
        for e in 2..2 + KEPT_EPOCHS as u64 {
            book.offer(&at(e, 20 + e));
        }
        assert!(book.of_epoch(0).is_none(), "the oldest epoch is forgotten");
        assert!(book.of_epoch(1).is_none());
        assert!(book.of_epoch(2).is_some());
    }
}
