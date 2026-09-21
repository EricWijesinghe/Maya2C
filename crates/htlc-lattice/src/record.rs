//! The lock record a chain stores, and its one encoding.
//!
//! The opening stays in the record after a claim. A counterparty's watcher
//! needs it to claim the other leg, and block bodies are prunable (invariant
//! 27): an opening that lived only in a transaction would be gone from a
//! pruned node while the other chain's timelock was still running. In state it
//! is also under the state root, so a light client can prove the revelation.

use crate::error::{Error, Result};
use crate::lock::{Lock, Unlock};
use crate::params::{COMMITMENT_BYTES, DIGEST_BYTES, OPENING_BYTES};
use crate::timelock::LockStatus;

/// Record format version.
///
/// 2 since hash locks: the lock and the unlock carry a family tag. No
/// version-1 record exists on any chain — HTLCs never activated before the
/// change — so version 1 is refused rather than migrated.
const RECORD_VERSION: u8 = 2;

/// A chain address.
pub type Address = [u8; DIGEST_BYTES];

const TAG_OPEN: u8 = 0;
const TAG_CLAIMED: u8 = 1;
const TAG_REFUNDED: u8 = 2;

/// Largest encoding: version, two addresses, amount, two heights, a lattice
/// lock, and a claimed settlement carrying an opening.
const MAX_BYTES: usize =
    1 + 2 * DIGEST_BYTES + 3 * 8 + 1 + COMMITMENT_BYTES + 1 + 8 + 1 + OPENING_BYTES;

/// How a lock ended, if it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settlement {
    /// Still escrowed.
    Open,
    /// Claimed at `height` with the unlock that did it.
    Claimed {
        /// Height of the claiming block.
        height: u64,
        /// The published preimage or opening.
        unlock: Unlock,
    },
    /// Refunded at `height`.
    Refunded {
        /// Height of the refunding block.
        height: u64,
    },
}

/// One lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockRecord {
    /// Who funded it, and who a refund repays.
    pub sender: Address,
    /// Who a claim pays. Fixed at lock time, so a copied opening can only pay
    /// the party the sender named.
    pub recipient: Address,
    /// Escrowed native base units.
    pub amount: u64,
    /// Height of the locking block.
    pub created_height: u64,
    /// First height at which a claim is refused and a refund admitted.
    pub expiry_height: u64,
    /// What a claim must unlock.
    pub lock: Lock,
    /// How it ended.
    pub settlement: Settlement,
}

impl LockRecord {
    /// The timelock status.
    #[must_use]
    pub const fn status(&self) -> LockStatus {
        match self.settlement {
            Settlement::Open => LockStatus::Locked,
            Settlement::Claimed { .. } => LockStatus::Claimed,
            Settlement::Refunded { .. } => LockStatus::Refunded,
        }
    }

    /// Value the record still holds: all of it until settled, none after.
    #[must_use]
    pub const fn escrowed(&self) -> u64 {
        match self.settlement {
            Settlement::Open => self.amount,
            Settlement::Claimed { .. } | Settlement::Refunded { .. } => 0,
        }
    }

    /// The published preimage or opening, once claimed.
    #[must_use]
    pub const fn revealed_unlock(&self) -> Option<&Unlock> {
        match &self.settlement {
            Settlement::Claimed { unlock, .. } => Some(unlock),
            Settlement::Open | Settlement::Refunded { .. } => None,
        }
    }

    /// The wire form.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAX_BYTES);
        out.push(RECORD_VERSION);
        out.extend_from_slice(&self.sender);
        out.extend_from_slice(&self.recipient);
        out.extend_from_slice(&self.amount.to_le_bytes());
        out.extend_from_slice(&self.created_height.to_le_bytes());
        out.extend_from_slice(&self.expiry_height.to_le_bytes());
        self.lock.encode_into(&mut out);
        match &self.settlement {
            Settlement::Open => out.push(TAG_OPEN),
            Settlement::Claimed { height, unlock } => {
                out.push(TAG_CLAIMED);
                out.extend_from_slice(&height.to_le_bytes());
                unlock.encode_into(&mut out);
            }
            Settlement::Refunded { height } => {
                out.push(TAG_REFUNDED);
                out.extend_from_slice(&height.to_le_bytes());
            }
        }
        out
    }

    /// Reads the wire form. Exact: trailing bytes are refused.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for a wrong version, tag, or length, and every
    /// refusal of [`Lock::decode`] and [`Unlock::decode`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader(bytes);
        if reader.take(1)?[0] != RECORD_VERSION {
            return Err(malformed("unknown version"));
        }
        let sender = reader.array()?;
        let recipient = reader.array()?;
        let amount = reader.u64()?;
        let created_height = reader.u64()?;
        let expiry_height = reader.u64()?;
        let (lock, used) = Lock::decode(reader.0)?;
        reader.take(used)?;
        let settlement = match reader.take(1)?[0] {
            TAG_OPEN => Settlement::Open,
            TAG_CLAIMED => {
                let height = reader.u64()?;
                let (unlock, used) = Unlock::decode(reader.0)?;
                reader.take(used)?;
                Settlement::Claimed { height, unlock }
            }
            TAG_REFUNDED => Settlement::Refunded {
                height: reader.u64()?,
            },
            _ => return Err(malformed("unknown settlement tag")),
        };
        if !reader.0.is_empty() {
            return Err(malformed("trailing bytes"));
        }
        Ok(Self {
            sender,
            recipient,
            amount,
            created_height,
            expiry_height,
            lock,
            settlement,
        })
    }
}

fn malformed(reason: &'static str) -> Error {
    Error::Malformed {
        what: "lock record",
        reason,
    }
}

/// A cursor that refuses to read past the end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if self.0.len() < count {
            return Err(malformed("truncated"));
        }
        let (head, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(head)
    }

    fn array(&mut self) -> Result<[u8; DIGEST_BYTES]> {
        self.take(DIGEST_BYTES)?
            .try_into()
            .map_err(|_| malformed("truncated"))
    }

    fn u64(&mut self) -> Result<u64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| malformed("truncated"))?;
        Ok(u64::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::lock::{HashFunction, Preimage};
    use crate::secret::LatticeSecret;

    fn lattice_lock() -> Lock {
        Lock::Lattice(
            LatticeSecret::from_entropy([5; 32])
                .commitment()
                .expect("commit"),
        )
    }

    fn record(settlement: Settlement) -> LockRecord {
        record_with(lattice_lock(), settlement)
    }

    fn record_with(lock: Lock, settlement: Settlement) -> LockRecord {
        LockRecord {
            sender: [1; 32],
            recipient: [2; 32],
            amount: 5_000,
            created_height: 7,
            expiry_height: 107,
            lock,
            settlement,
        }
    }

    #[test]
    fn every_settlement_round_trips() {
        let opening = LatticeSecret::from_entropy([5; 32]).opening();
        for settlement in [
            Settlement::Open,
            Settlement::Claimed {
                height: 50,
                unlock: Unlock::Opening(opening),
            },
            Settlement::Refunded { height: 107 },
        ] {
            let original = record(settlement);
            assert_eq!(
                LockRecord::decode(&original.encode()).expect("decode"),
                original
            );
        }
    }

    #[test]
    fn hash_lock_records_round_trip_for_every_function() {
        let preimage = Preimage::new([9; 32]);
        for function in [
            HashFunction::Sha3_256,
            HashFunction::Blake3,
            HashFunction::Sha256,
        ] {
            for settlement in [
                Settlement::Open,
                Settlement::Claimed {
                    height: 60,
                    unlock: Unlock::Preimage(preimage.clone()),
                },
            ] {
                let original = record_with(Lock::hash(function, &preimage), settlement);
                assert_eq!(
                    LockRecord::decode(&original.encode()).expect("decode"),
                    original
                );
            }
        }
    }

    #[test]
    fn a_version_1_record_is_refused() {
        let mut bytes = record(Settlement::Open).encode();
        bytes[0] = 1;
        assert!(LockRecord::decode(&bytes).is_err());
    }

    #[test]
    fn a_settled_record_escrows_nothing() {
        assert_eq!(record(Settlement::Open).escrowed(), 5_000);
        assert_eq!(record(Settlement::Refunded { height: 1 }).escrowed(), 0);
    }

    #[test]
    fn trailing_and_truncated_bytes_are_refused() {
        let mut bytes = record(Settlement::Open).encode();
        bytes.push(0);
        assert!(LockRecord::decode(&bytes).is_err());
        bytes.truncate(bytes.len() - 2);
        assert!(LockRecord::decode(&bytes).is_err());
        assert!(LockRecord::decode(&[]).is_err());
    }
}
