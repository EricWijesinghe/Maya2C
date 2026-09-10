//! The bounds no fee configuration may escape.
//!
//! Compiled in, appearing in no configuration key and reachable by no
//! transaction — the arrangement `governance/src/limits.rs` has for the rules
//! that govern governance, and for the same reason. When the fee market is
//! activated, its parameters will be governable (invariant 17), and a
//! governance proposal must not be able to set a change rate that swings fees
//! to zero in a block, or a base-fee floor of zero that makes spam free.
//! [`crate::FeeConfig::validate`] checks every configuration against these.

/// The smallest change denominator: the base fee moves by at most `1/8` of
/// itself per block. EIP-1559's value, and the fastest this chain will allow.
pub const MIN_CHANGE_DENOMINATOR: u64 = 8;

/// The largest change denominator: at least `1/1024` per block, so the fee
/// cannot be configured into a constant.
pub const MAX_CHANGE_DENOMINATOR: u64 = 1024;

/// The lowest base fee floor, in base units per byte.
///
/// Not zero. A zero floor lets a quiet chain decay to a free one, and a free
/// byte on a chain whose signatures are 11,165 bytes is a storage subsidy for
/// whoever wants it.
pub const MIN_BASE_FEE_FLOOR: u64 = 1;

/// The smallest block-size target a configuration may choose, in bytes.
///
/// Below this, a single hybrid-signed transaction (~11.5 KB) is over target on
/// its own, and the base fee ratchets up on every non-empty block.
pub const MIN_TARGET_BLOCK_BYTES: u64 = 64 * 1024;

/// The largest block-size target, in bytes.
pub const MAX_TARGET_BLOCK_BYTES: u64 = 16 * 1024 * 1024;

/// The most of each base fee that may go to the treasury, in basis points.
///
/// The rest is burned. A treasury share of 100% would turn the burn — the only
/// thing that makes the fee a cost to the chain rather than a transfer — off.
pub const MAX_TREASURY_BPS: u64 = 5_000;

/// One hundred percent, in basis points.
pub const BPS: u64 = 10_000;
