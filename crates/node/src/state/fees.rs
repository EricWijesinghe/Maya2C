//! The fee market in the node (ADR-029): a base fee per serialized byte that
//! burns, and tips that pay validators.
//!
//! # A fee is an output, not a field
//!
//! A transaction pays by including an ordinary signed output to
//! [`FEE_COLLECTOR`]. No wire change: every transaction format this chain
//! accepts (hybrid, suite-tagged, multisig) already signs its outputs, so the
//! amount is the sender's explicit, signed consent — the property EIP-1559's
//! `max_fee` exists for — and the fee is exact rather than a cap. What the
//! collector receives above `base_fee × size` is the tip.
//!
//! Per block: the base-fee part of everything collected moves to
//! [`FEE_SINK`](super::shielded::FEE_SINK) (burned: supply conserved, circulation falls) and the base fee
//! steps toward `target_block_bytes` by `maya_fee_market::next_base_fee`. The
//! tips stay in the collector until the staking epoch ends, when they are the
//! reward pool (`staking_exec`).
//!
//! # Presence is activation
//!
//! `k:fee` exists only where genesis configures fees. Without it nothing here
//! runs and no root moves.

use maya_dex::Amount;
use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;

/// The fee state record. Under the staking layer's prefix: fees and staking
/// are one economic subsystem, and one more layer would buy nothing.
pub const FEE_KEY: &[u8] = b"k:fee";

/// Where fees are paid. A derived address: nobody holds its key.
pub const FEE_COLLECTOR: Address = [
    0x6d, 0x61, 0x79, 0x61, 0x32, 0x63, 0x2f, 0x66, 0x65, 0x65, 0x2f, 0x63, 0x6f, 0x6c, 0x6c, 0x65,
    0x63, 0x74, 0x6f, 0x72, 0x2f, 0x76, 0x31, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The fee market's parameters and running state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeRecord {
    /// Current base fee, per serialized byte.
    pub base_fee: Amount,
    /// Floor for the base fee.
    pub min_base_fee: Amount,
    /// Block size, in bytes, the base fee steers toward.
    pub target_block_bytes: u64,
    /// The base fee moves by at most `1 / change_denominator` per block.
    pub change_denominator: u64,
}

impl FeeRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        // Version 2: the two fees are 32-byte amounts (ADR-066). Version 1
        // carried them as u64, so an old record fails to decode rather than
        // being misread.
        let mut out = vec![2u8];
        out.extend_from_slice(&self.base_fee.to_le_bytes());
        out.extend_from_slice(&self.min_base_fee.to_le_bytes());
        out.extend_from_slice(&self.target_block_bytes.to_le_bytes());
        out.extend_from_slice(&self.change_denominator.to_le_bytes());
        out
    }

    /// Decodes a record.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong version or length.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != 2 {
            return Err(NodeError::Decode("fee record version".to_string()));
        }
        let record = Self {
            base_fee: Amount::from_le_bytes(r.read_array::<32>()?),
            min_base_fee: Amount::from_le_bytes(r.read_array::<32>()?),
            target_block_bytes: r.read_u64()?,
            change_denominator: r.read_u64()?,
        };
        r.finish()?;
        Ok(record)
    }

    /// What a transaction of `size` bytes must pay at this base fee.
    #[must_use]
    pub fn required(&self, size: usize) -> Amount {
        // A fee near 2^256 times a size can overflow; nobody can pay that, so it
        // saturates to a requirement no balance meets rather than wrapping.
        self.base_fee
            .checked_mul(Amount::from_u128(size as u128))
            .unwrap_or(Amount::MAX)
    }

    /// The record after a block of `block_bytes`.
    #[must_use]
    pub fn next(&self, block_bytes: u64) -> Self {
        Self {
            base_fee: maya_fee_market::next_base_fee(
                self.base_fee,
                block_bytes,
                self.target_block_bytes,
                self.change_denominator,
                self.min_base_fee,
            ),
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn record() -> FeeRecord {
        FeeRecord {
            base_fee: Amount::from_u64(10),
            min_base_fee: Amount::from_u64(1),
            target_block_bytes: 1_000,
            change_denominator: 8,
        }
    }

    #[test]
    fn the_record_round_trips() {
        assert_eq!(FeeRecord::decode(&record().encode()).unwrap(), record());
    }

    #[test]
    fn the_base_fee_rises_over_target_falls_under_and_keeps_its_floor() {
        assert!(record().next(2_000).base_fee > Amount::from_u64(10));
        assert!(record().next(0).base_fee < Amount::from_u64(10));
        let floor = FeeRecord {
            base_fee: Amount::from_u64(1),
            ..record()
        };
        assert_eq!(floor.next(0).base_fee, Amount::from_u64(1));
    }

    #[test]
    fn the_collector_is_not_the_sink_nor_a_zero_address() {
        assert_ne!(FEE_COLLECTOR, crate::state::shielded::FEE_SINK);
        assert_ne!(FEE_COLLECTOR, [0; 32]);
    }
}
