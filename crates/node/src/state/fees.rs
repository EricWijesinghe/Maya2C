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
//! [`FEE_SINK`] (burned: supply conserved, circulation falls) and the base fee
//! steps toward `target_block_bytes` by `maya_fee_market::next_base_fee`. The
//! tips stay in the collector until the staking epoch ends, when they are the
//! reward pool (`staking_exec`).
//!
//! # Presence is activation
//!
//! `k:fee` exists only where genesis configures fees. Without it nothing here
//! runs and no root moves.

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
    pub base_fee: u64,
    /// Floor for the base fee.
    pub min_base_fee: u64,
    /// Block size, in bytes, the base fee steers toward.
    pub target_block_bytes: u64,
    /// The base fee moves by at most `1 / change_denominator` per block.
    pub change_denominator: u64,
}

impl FeeRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![1u8];
        for v in [
            self.base_fee,
            self.min_base_fee,
            self.target_block_bytes,
            self.change_denominator,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Decodes a record.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on a wrong version or length.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != 1 {
            return Err(NodeError::Decode("fee record version".to_string()));
        }
        let record = Self {
            base_fee: r.read_u64()?,
            min_base_fee: r.read_u64()?,
            target_block_bytes: r.read_u64()?,
            change_denominator: r.read_u64()?,
        };
        r.finish()?;
        Ok(record)
    }

    /// What a transaction of `size` bytes must pay at this base fee.
    #[must_use]
    pub fn required(&self, size: usize) -> u128 {
        u128::from(self.base_fee) * size as u128
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
            base_fee: 10,
            min_base_fee: 1,
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
        assert!(record().next(2_000).base_fee > 10);
        assert!(record().next(0).base_fee < 10);
        let floor = FeeRecord {
            base_fee: 1,
            ..record()
        };
        assert_eq!(floor.next(0).base_fee, 1);
    }

    #[test]
    fn the_collector_is_not_the_sink_nor_a_zero_address() {
        assert_ne!(FEE_COLLECTOR, crate::state::shielded::FEE_SINK);
        assert_ne!(FEE_COLLECTOR, [0; 32]);
    }
}
