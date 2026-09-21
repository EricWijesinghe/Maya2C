//! On-chain governance: what the chain stores about proposals, stake, and the
//! rules currently in force.
//!
//! The decisions live in [`maya_governance`]. This module is the part that has
//! to know about storage — how a record is laid out, what key it files under,
//! and how it enters the Merkle state root.
//!
//! ## Voting power
//!
//! Two sources, added:
//!
//! - **Locked stake.** Native coin debited into a lock record with an unlock
//!   height. The lock must outlive the proposal's *execution*, not merely its
//!   vote — otherwise the cheapest way to decide something is to acquire
//!   weight, vote, and be gone before the decision binds anyone who stayed.
//! - **Recent work.** See [`work`]. This chain has no block reward and no miner
//!   field in the header, so there was no on-chain record of who mined
//!   anything; [`crate::core::governance_payload::WorkClaim`] creates one
//!   without touching the proof-of-work preimage.
//!
//! Quorum is measured against locked stake alone, for the reason
//! [`maya_governance::tally`] gives: work credit decays per address at a
//! different moment for each address, so a chain-wide total of it would drift
//! from the sum of its parts.
//!
//! ## One keyspace, one prefix
//!
//! Everything here lives under `g:`, joining the trading subsystem's `d:` and
//! the oracle's `o:`. No pre-existing prefix begins with that byte, so one scan
//! collects the whole subsystem — which is what lets it fold into the state
//! root as a single layer and journal into the undo record with no new section
//! at all.

pub mod record;
pub mod work;

pub use record::{LockRecord, ParameterTable, ProposalRecord, Totals};
pub use work::{WORK_HALF_LIFE_BLOCKS, WorkRecord};

/// Key prefix shared by every record the governance subsystem owns.
pub(crate) const GOVERNANCE_PREFIX: &[u8] = b"g:";

/// Storage key for the parameter table.
///
/// One blob rather than one key per parameter. The table is ten entries, it is
/// read whole on every block that consults any rule, and splitting it would
/// turn one lookup into ten.
pub(crate) const PARAMETERS_KEY: &[u8] = b"g:params";

/// Storage key for the chain-wide totals that quorum is measured against.
pub(crate) const TOTALS_KEY: &[u8] = b"g:totals";

/// Prefix under which each proposal's record lives.
pub(crate) const PROPOSAL_PREFIX: &[u8] = b"g:prop:";

/// Prefix under which each cast ballot lives: `g:vote:<proposal><voter>`.
///
/// Presence is the whole record — the value is the choice, but what stops a
/// second vote is the key already existing. Storing ballots as one blob per
/// proposal would make double-vote detection a scan of everyone who has voted.
pub(crate) const BALLOT_PREFIX: &[u8] = b"g:vote:";

/// Prefix under which each address's locked stake lives.
pub(crate) const LOCK_PREFIX: &[u8] = b"g:lock:";

/// Prefix under which each address's work credit lives.
pub(crate) const WORK_PREFIX: &[u8] = b"g:work:";

/// Storage key for one proposal.
#[must_use]
pub fn proposal_key(id: &[u8; 32]) -> Vec<u8> {
    join(PROPOSAL_PREFIX, &[id.as_slice()])
}

/// Storage key for one address's ballot on one proposal.
#[must_use]
pub fn ballot_key(proposal: &[u8; 32], voter: &[u8; 32]) -> Vec<u8> {
    join(BALLOT_PREFIX, &[proposal.as_slice(), voter.as_slice()])
}

/// Storage key for one address's locked stake.
#[must_use]
pub fn lock_key(address: &[u8; 32]) -> Vec<u8> {
    join(LOCK_PREFIX, &[address.as_slice()])
}

/// Storage key for one address's work credit.
#[must_use]
pub fn work_key(address: &[u8; 32]) -> Vec<u8> {
    join(WORK_PREFIX, &[address.as_slice()])
}

fn join(prefix: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let length = prefix.len() + parts.iter().map(|part| part.len()).sum::<usize>();
    let mut key = Vec::with_capacity(length);
    key.extend_from_slice(prefix);
    for part in parts {
        key.extend_from_slice(part);
    }
    key
}

/// Derives a proposal identifier from the transaction that opened it.
///
/// The proposer and nonce together are unique, so no two proposals can collide
/// and the proposer can compute the identifier before submitting — which they
/// need in order to campaign for it.
#[must_use]
pub fn derive_proposal_id(proposer: &[u8; 32], nonce: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("maya-governance proposal id v1");
    hasher.update(proposer);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}
