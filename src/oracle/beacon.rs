//! The randomness beacon.
//!
//! ## The construction
//!
//! ```text
//! alpha_n   = previous_beacon ‖ height_n
//! output_n  = VRF(sk_proposer, alpha_n)
//! beacon_n  = H(previous_beacon ‖ output_n)
//! ```
//!
//! Three parties would have to cooperate to steer it, and none of them can act
//! alone:
//!
//! - **The proposer cannot choose its output.** A VRF is unique per
//!   (key, input), so for a fixed `alpha` there is exactly one proof it can
//!   produce. Grinding requires re-keying, which changes its address and takes
//!   it out of the registry.
//! - **The proposer cannot choose `alpha`.** Both halves are fixed before it
//!   acts: the previous beacon is committed state, and the height is the block
//!   being built.
//! - **The miner cannot compute the output.** It does not hold the key.
//!
//! ## What a miner *can* still do, stated plainly
//!
//! Include the proposer's proof, or leave it out. Leaving it out yields
//! [`fallback_beacon`] instead, which is a different value — so a miner holding
//! a block choice gets **one bit of influence over each block it mines**, free,
//! and can retry that choice as often as it mines.
//!
//! That is bounded and it is not zero. Two things reduce what it is worth:
//!
//! 1. The accumulator chains, so a bit chosen now is composed with every future
//!    proposer's output. Steering a value k blocks out costs influence over all
//!    k, and the miner holds influence over only the blocks it wins.
//! 2. The fallback is *deterministic*, so the miner chooses between two known
//!    values rather than searching a space.
//!
//! The fix, if that bit ever matters for an application, is to move the proof
//! into the block header so that choosing between the two values costs a full
//! re-mine rather than nothing. That is a change to [`crate::core::BlockHeader`]
//! — and therefore to the proof-of-work preimage, the miner, the pool protocol,
//! and the GPU kernel — which is why it is named here rather than done.
//!
//! **Do not use this beacon for a lottery whose payout exceeds a block reward
//! without reading the paragraph above.**
//!
//! ## Liveness does not depend on the proposer
//!
//! An absent or invalid proof is not an error. The chain folds
//! [`fallback_beacon`] and moves on. The alternative — refusing blocks with no
//! beacon proof — would hand any single authority the power to halt the chain
//! by going offline, which is a worse property than the one bit above.

use crate::core::codec::ByteReader;
use crate::error::Result;

/// Encoded size of a [`BeaconState`].
pub const BEACON_STATE_LEN: usize = 32 + 8 + 8;

/// The accumulator, and how it got there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BeaconState {
    /// Current randomness.
    pub value: [u8; 32],
    /// Height whose block last folded into it.
    pub height: u64,
    /// How many real VRF outputs have been folded in.
    ///
    /// Distinct from `height`, and the gap between them is the count of blocks
    /// that fell back. Exposed because "how much of this randomness came from a
    /// VRF" is the question an application ought to ask before trusting it, and
    /// there is no way to ask it from the value alone.
    pub contributions: u64,
}

impl BeaconState {
    /// The beacon before any block has contributed.
    ///
    /// Seeded from the chain identifier rather than from zero, so two networks
    /// with identical block histories do not share a randomness sequence.
    #[must_use]
    pub fn genesis(chain_id: &str) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key("maya-oracle beacon genesis v1");
        hasher.update(chain_id.as_bytes());
        Self {
            value: *hasher.finalize().as_bytes(),
            height: 0,
            contributions: 0,
        }
    }

    /// Folds a VRF output into the accumulator.
    ///
    /// The previous value is hashed alongside the new output rather than
    /// replaced by it. Replacement would make the beacon a function of one
    /// proposer's key at one height; chaining makes every value a function of
    /// every contribution before it, so steering it requires steering all of
    /// them.
    #[must_use]
    pub fn fold(&self, output: &[u8], height: u64) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key("maya-oracle beacon fold v1");
        hasher.update(&self.value);
        hasher.update(output);
        hasher.update(&height.to_le_bytes());
        Self {
            value: *hasher.finalize().as_bytes(),
            height,
            contributions: self.contributions.saturating_add(1),
        }
    }

    /// Advances the accumulator for a block that carried no valid proof.
    ///
    /// Still advances: a beacon that stalled would repeat a value across
    /// blocks, and an application sampling it once per block would draw the
    /// same number twice.
    #[must_use]
    pub fn fold_fallback(&self, height: u64) -> Self {
        Self {
            value: fallback_beacon(&self.value, height),
            height,
            contributions: self.contributions,
        }
    }

    /// Encodes the state.
    #[must_use]
    pub fn encode(&self) -> [u8; BEACON_STATE_LEN] {
        let mut buf = [0u8; BEACON_STATE_LEN];
        buf[..32].copy_from_slice(&self.value);
        buf[32..40].copy_from_slice(&self.height.to_le_bytes());
        buf[40..].copy_from_slice(&self.contributions.to_le_bytes());
        buf
    }

    /// Decodes a stored state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`](crate::error::NodeError::Decode) if the record is not exactly
    /// [`BEACON_STATE_LEN`] bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let state = Self {
            value: reader.read_array::<32>()?,
            height: reader.read_u64()?,
            contributions: reader.read_u64()?,
        };
        reader.finish()?;
        Ok(state)
    }

    /// Hashes the state into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-oracle beacon leaf v1");
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}

/// The VRF input for a block.
///
/// Both halves are fixed before the proposer acts, which is what stops it
/// choosing its own output: the previous beacon is committed state, and the
/// height is the block being built. A proposer that dislikes the result has
/// nothing to vary.
///
/// Height is included as well as the previous beacon so that a proof made for
/// one height cannot be replayed into another — without it, a proposer could
/// hold a proof back and submit it into whichever later block suited them.
#[must_use]
pub fn beacon_alpha(previous: &[u8; 32], height: u64) -> [u8; 40] {
    let mut alpha = [0u8; 40];
    alpha[..32].copy_from_slice(previous);
    alpha[32..].copy_from_slice(&height.to_le_bytes());
    alpha
}

/// The accumulator value for a block with no valid proof.
///
/// A hash of the previous value and the height, under its own domain — so it is
/// deterministic, distinct from any value a real proof could produce, and
/// unpredictable to anyone who cannot predict the previous value.
#[must_use]
pub fn fallback_beacon(previous: &[u8; 32], height: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("maya-oracle beacon fallback v1");
    hasher.update(previous);
    hasher.update(&height.to_le_bytes());
    *hasher.finalize().as_bytes()
}
