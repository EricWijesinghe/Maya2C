//! A journalled swap: which two locks, which role, how it ended.

use serde::{Deserialize, Serialize};

use maya_htlc_lattice::CommitmentId;

/// A lock identifier.
pub type LockId = [u8; 32];

/// Which of the watcher's two chains a lock is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainSide {
    /// The chain this watcher's node serves.
    Maya,
    /// The other chain of the swap, running the same verifier.
    Counterparty,
}

/// Which side of the swap this watcher is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Holds the secret. Locks first with the longer expiry; claims first,
    /// which reveals the opening — and must not reveal late.
    Initiator,
    /// Does not hold the secret. Locks second with the shorter expiry; claims
    /// with the opening the initiator's claim published.
    Responder,
}

/// One lock of a swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leg {
    /// Which chain.
    pub side: ChainSide,
    /// Which lock.
    #[serde(with = "hex32")]
    pub lock_id: LockId,
    /// Its expiry height on that chain, as checked at registration.
    pub expiry_height: u64,
}

/// How a swap ended, from this watcher's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// We claimed the inbound lock and the counterparty claimed ours.
    Swapped,
    /// Neither lock was claimed; both refunded to their senders.
    Unwound,
    /// We claimed the inbound lock and our outbound lock refunded to us: the
    /// counterparty failed to claim in time.
    KeptBoth,
    /// Our outbound lock was claimed and the inbound lock refunded to its
    /// sender. The outcome everything in [`crate::policy`] exists to prevent.
    Lost,
}

/// Whether a swap still needs watching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Still settling.
    Active,
    /// Both legs settled and confirmed.
    Finished(Outcome),
}

/// A swap this watcher is responsible for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    /// The commitment both locks carry.
    #[serde(with = "hex32")]
    pub commitment_id: CommitmentId,
    /// Our role.
    pub role: Role,
    /// The lock that pays us. `None` for an initiator until the responder's
    /// lock has been checked: the initiator funds first, and its refund has to
    /// be watched from that moment, not from when somebody answers.
    pub inbound: Option<Leg>,
    /// The lock we funded.
    pub outbound: Leg,
    /// Where it stands.
    pub phase: Phase,
}

/// 32-byte arrays as hex strings, so an operator can read the journal.
mod hex32 {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = hex::decode(&text).map_err(D::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| D::Error::custom("expected 32 bytes"))
    }
}
