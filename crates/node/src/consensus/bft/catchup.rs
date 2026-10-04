//! Catching up to a checkpoint (ADR-038, mainnet gate 8).
//!
//! A node that fell more than the engine's 50-round window behind cannot
//! derive the blocks it missed. It asks a peer for the newest checkpoint —
//! a block a quorum of the committee attested — checks the quorum against
//! the committee it already knows, fetches the missing blocks, checks they
//! hash-chain to the attested block, and **re-executes** each through
//! [`Chain::insert_block`], which refuses a state root its own execution
//! does not reproduce (invariant 24). The peer is trusted for nothing: the
//! quorum names the chain, re-execution checks the state. Afterwards the
//! node is within the window and the engine fetches the last rounds itself.

use crate::consensus::{Chain, InsertOutcome};
use crate::core::{Block, ChainTag};
use crate::crypto::keys::VerifyingKey;
use crate::error::{NodeError, Result};

use super::attest::Checkpoint;

/// Where a node that fell behind gets a checkpoint and the blocks up to it.
/// Every answer is untrusted; [`catch_up`] checks all of it.
pub trait CheckpointSource {
    /// The source's newest checkpoint, if it holds one.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn checkpoint(&self) -> Result<Option<Checkpoint>>;

    /// The source's newest checkpoint of `epoch`, if it still holds one:
    /// how a node behind an epoch boundary crosses it (see [`fetch`]).
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn checkpoint_of(&self, epoch: u64) -> Result<Option<Checkpoint>>;

    /// The active-chain block at `height`.
    ///
    /// # Errors
    ///
    /// Any failure reaching the source.
    fn block(&self, height: u64) -> Result<Block>;
}

/// Where this node's chain stands, read under the chain lock so the slow
/// part — fetching from a peer — can run without it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// The chain the checkpoint must name.
    pub tag: ChainTag,
    /// Current height.
    pub height: u64,
    /// Current tip.
    pub tip: [u8; 32],
    /// The staking epoch the chain's state is in: whose committee this node
    /// trusts. Zero on a chain without staking.
    pub epoch: u64,
}

impl Position {
    /// `chain`'s position now.
    #[must_use]
    pub fn of(chain: &Chain) -> Self {
        Self {
            tag: ChainTag::from_genesis(chain.genesis()),
            height: chain.height(),
            tip: chain.tip(),
            // A state that cannot be read here only names a wrong epoch, and
            // a checkpoint fetched for it fails verification: safe.
            epoch: chain
                .state()
                .committed_staking()
                .ok()
                .flatten()
                .map_or(0, |r| r.staking.epoch),
        }
    }
}

/// Imports blocks from `source` up to its newest checkpoint, returning how
/// many were imported (zero when already there).
///
/// `committee` is the committee this node already trusts — the genesis or
/// staking committee in its own state. A checkpoint signed by another
/// committee is refused. To cross a membership change, [`fetch`] stops at the
/// final checkpoint of this node's own epoch: the boundary block below it
/// writes the next committee into state, and the next call trusts that
/// one. Call again until nothing is imported. `weights` are that
/// committee's voting weights on a stake-weighted chain (ADR-040 part 2),
/// `None` where every member counts one.
///
/// # Errors
///
/// As [`fetch`] and [`import`].
pub fn catch_up(
    chain: &mut Chain,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
    weights: Option<&[u64]>,
) -> Result<u64> {
    let blocks = fetch(Position::of(chain), source, committee, weights)?;
    import(chain, blocks)
}

/// The final checkpoint of `from`'s own epoch, for crossing into the next
/// one. Loud rather than empty when it cannot move this node forward: an
/// empty answer would leave a node at an epoch boundary looking caught up
/// while it waits for a boundary block it never imports.
fn own_epoch_final(
    from: Position,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
    weights: Option<&[u64]>,
) -> Result<Checkpoint> {
    let Some(own) = source.checkpoint_of(from.epoch)? else {
        return Err(NodeError::Network(format!(
            "the peer holds no checkpoint of epoch {}, which it keeps for {} epochs:              restore from a snapshot (--bootstrap-from) instead",
            from.epoch,
            super::attest::KEPT_EPOCHS
        )));
    };
    own.verify(&from.tag, committee, weights)?;
    if own.height <= from.height {
        return Err(NodeError::Network(format!(
            "stuck at the end of epoch {}: the peer's final checkpoint of it is at height {},              not past this node's {}; try another peer",
            from.epoch, own.height, from.height
        )));
    }
    Ok(own)
}

/// Fetches the blocks after `from` up to `source`'s newest checkpoint, checked
/// to carry a quorum of `committee` for `from.tag` and to form one hash chain
/// from `from.tip` to the attested block. Empty when there is nothing newer.
///
/// # Errors
///
/// [`NodeError::Decode`] for a checkpoint without a quorum or blocks that do
/// not chain to it; errors from `source` as they come.
pub fn fetch(
    from: Position,
    source: &impl CheckpointSource,
    committee: &[VerifyingKey],
    weights: Option<&[u64]>,
) -> Result<Vec<Block>> {
    let Some(newest) = source.checkpoint()? else {
        return Ok(Vec::new());
    };
    // The network is past this node's epoch: its committee cannot check the
    // newer checkpoint, so go as far as this epoch's own final one first.
    let checkpoint = if newest.epoch > from.epoch {
        own_epoch_final(from, source, committee, weights)?
    } else {
        newest
    };
    checkpoint.verify(&from.tag, committee, weights)?;
    if checkpoint.height <= from.height {
        return Ok(Vec::new());
    }
    let mut parent = from.tip;
    let mut blocks = Vec::new();
    for height in from.height + 1..=checkpoint.height {
        let block = source.block(height)?;
        if block.header.prev_hash != parent {
            return Err(NodeError::Decode(format!(
                "catch-up: block {height} does not extend the one before it"
            )));
        }
        parent = block.header.id();
        blocks.push(block);
    }
    if parent != checkpoint.block {
        return Err(NodeError::Decode(
            "catch-up: the blocks do not lead to the attested block".to_string(),
        ));
    }
    Ok(blocks)
}

/// Re-executes `blocks` onto `chain` through [`Chain::insert_block`], which
/// refuses a state root its own execution does not reproduce (invariant 24).
/// Returns how many were imported.
///
/// # Errors
///
/// A block that does not extend the tip — the chain moved since
/// [`fetch`] — or fails to execute. Blocks imported before it stay.
pub fn import(chain: &mut Chain, blocks: Vec<Block>) -> Result<u64> {
    let mut imported = 0;
    for block in blocks {
        match chain.insert_block(block)? {
            InsertOutcome::Extended { .. } => imported += 1,
            other => {
                return Err(NodeError::Decode(format!(
                    "catch-up: a block did not extend the tip: {other:?}"
                )));
            }
        }
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::*;
    use crate::consensus::bft::attest::Attestation;
    use crate::crypto::keys::{SigningKey, signing_key_from_seed};

    const TAG: ChainTag = ChainTag::from_genesis([7; 32]);

    fn keys() -> (Vec<SigningKey>, Vec<VerifyingKey>) {
        let signing: Vec<SigningKey> = (1..=4u8)
            .map(|i| signing_key_from_seed(&[i; 32]).unwrap())
            .collect();
        let verifying = signing.iter().map(SigningKey::verifying_key).collect();
        (signing, verifying)
    }

    /// Three of four sign: a quorum.
    fn checkpoint(signing: &[SigningKey], epoch: u64, height: u64) -> Checkpoint {
        let signatures = (0..3u16)
            .map(|v| {
                let a =
                    Attestation::sign(&signing[usize::from(v)], v, &TAG, epoch, height, [9; 32])
                        .unwrap();
                (v, a.signature)
            })
            .collect::<BTreeMap<_, _>>();
        Checkpoint {
            epoch,
            height,
            block: [9; 32],
            signatures,
        }
    }

    struct Source {
        newest: Checkpoint,
        of_epoch: Option<Checkpoint>,
        asked: RefCell<Vec<u64>>,
    }

    impl CheckpointSource for Source {
        fn checkpoint(&self) -> Result<Option<Checkpoint>> {
            Ok(Some(self.newest.clone()))
        }
        fn checkpoint_of(&self, epoch: u64) -> Result<Option<Checkpoint>> {
            self.asked.borrow_mut().push(epoch);
            Ok(self.of_epoch.clone())
        }
        fn block(&self, height: u64) -> Result<Block> {
            Err(NodeError::Network(format!("block {height} requested")))
        }
    }

    fn at(height: u64, epoch: u64) -> Position {
        Position {
            tag: TAG,
            height,
            tip: [0; 32],
            epoch,
        }
    }

    #[test]
    fn behind_a_boundary_it_fetches_up_to_its_own_epochs_final_checkpoint() {
        let (signing, committee) = keys();
        let source = Source {
            newest: checkpoint(&signing, 2, 90), // a committee it cannot check
            of_epoch: Some(checkpoint(&signing, 1, 60)),
            asked: RefCell::default(),
        };
        let err = fetch(at(40, 1), &source, &committee, None).unwrap_err();
        assert_eq!(
            *source.asked.borrow(),
            [1],
            "asked for its own epoch's final checkpoint"
        );
        assert!(err.to_string().contains("block 41 requested"), "{err}");
    }

    #[test]
    fn in_the_same_epoch_it_never_asks_for_an_epochs_final_checkpoint() {
        let (signing, committee) = keys();
        let source = Source {
            newest: checkpoint(&signing, 1, 50),
            of_epoch: None,
            asked: RefCell::default(),
        };
        assert!(
            fetch(at(50, 1), &source, &committee, None)
                .unwrap()
                .is_empty()
        );
        assert!(source.asked.borrow().is_empty());
    }

    #[test]
    fn a_peer_without_the_epoch_or_short_of_the_boundary_is_an_error_not_silence() {
        let (signing, committee) = keys();
        let forgotten = Source {
            newest: checkpoint(&signing, 9, 900),
            of_epoch: None,
            asked: RefCell::default(),
        };
        let err = fetch(at(40, 1), &forgotten, &committee, None).unwrap_err();
        assert!(err.to_string().contains("snapshot"), "{err}");
        let short = Source {
            newest: checkpoint(&signing, 2, 90),
            of_epoch: Some(checkpoint(&signing, 1, 59)),
            asked: RefCell::default(),
        };
        let err = fetch(at(59, 1), &short, &committee, None).unwrap_err();
        assert!(
            err.to_string().contains("stuck at the end of epoch 1"),
            "{err}"
        );
    }
}
