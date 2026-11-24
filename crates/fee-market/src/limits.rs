//! The bounds no fee configuration may escape.
//!
//! Compiled in, appearing in no configuration key and reachable by no
//! transaction — the arrangement `crates/governance/src/limits.rs` has for the rules
//! that govern governance, and for the same reason. When the fee market is
//! activated, its parameters will be governable (invariant 17), and a
//! governance proposal must not be able to set a change rate that swings fees
//! to zero in a block, or a base-fee floor of zero that makes spam free.
//! [`crate::FeeConfig::validate`] checks every configuration against these.

use maya_amount::Amount;

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
pub const MIN_BASE_FEE_FLOOR: Amount = Amount::from_u64(1);

/// The highest base fee, in base units per byte. ADR-076.
///
/// Approved by Eric (ADR-076, option B): 0.1 MAYA for one standard
/// transfer of 13,215 bytes (`reports/13-pq-weight.md`) at 24 decimals
/// (`apps/wallet-gui/core/src/amount.rs::DECIMALS`). 0.1 MAYA = 10^23 base units,
/// and 10^23 / 13_215 = 7_567_158_531_971_244_797. The rejected options were
/// 0.01 MAYA = 756_715_853_197_124_479 and 1 MAYA = 75_671_585_319_712_447_975
/// per byte. Changing it changes genesis consensus.
pub const MAX_BASE_FEE_PER_BYTE: Amount = Amount::from_u128(7_567_158_531_971_244_797);

/// The smallest block-size target a configuration may choose, in bytes.
///
/// Below this, a single hybrid-signed transaction (~11.5 KB) is over target on
/// its own, and the base fee ratchets up on every non-empty block.
pub const MIN_TARGET_BLOCK_BYTES: u64 = 64 * 1024;

/// The largest block-size target, in bytes: half the block cap (ADR-072).
///
/// A target at or above `MAX_BLOCK_BYTES` could never be exceeded, so the
/// base fee could never rise and spam would stay at the floor. At half, a
/// full block is 2x target and moves the fee by the full 1/D step.
pub const MAX_TARGET_BLOCK_BYTES: u64 = MAX_BLOCK_BYTES / 2;

/// The largest block a chain accepts, in wire bytes (ADR-072).
///
/// Provisional, pending measurement on the launch host. Two bounds fix it.
/// Below: one maximal transaction (`MAX_TX_BYTES`, 4 MiB) plus the block's
/// own framing must fit alone, or a valid transaction could never be
/// included and would hold up every one queued behind it. Above: a block at
/// the cap must still travel in one 8 MiB gossip frame. 5 MiB sits between
/// them, and admits the 400-transfer block measured in
/// `reports/bench/2026-10-25-tps-series.md` (about 4.3 MiB) with room to spare.
pub const MAX_BLOCK_BYTES: u64 = 5 * 1024 * 1024;

/// The most bytes governance can ever set a block to (`block.max_bytes`,
/// ADR-155 §3, B1). [`MAX_BLOCK_BYTES`] is only where the value starts.
///
/// The ceiling is what every transport of the same build carries whole: one
/// 8 MiB gossip frame with a mebibyte left for its framing, and a 24 MiB sync
/// response three times over (the node asserts both at compile time). Raising
/// it is a release with larger or chunked frames, a scheduled upgrade, never a
/// change of format.
pub const BLOCK_BYTES_CEILING: u64 = 7 * 1024 * 1024;

/// Fewest wire bytes one transaction entry can take inside a block: its 8-byte
/// length prefix and a one-byte version. A decoder bound, far below any valid
/// transaction (a hybrid-signed transfer is about 11.5 KB).
pub const MIN_TX_ENTRY_BYTES: u64 = 8 + 1;

/// The most transactions governance can ever allow in one block
/// (`block.max_transactions`, NEW-4): as many as the byte ceiling could hold
/// at [`MIN_TX_ENTRY_BYTES`] each, so the count can never be the bound that a
/// smaller transaction format (witness separation, aggregation) runs into
/// first. The decoder refuses a count above it before allocating.
pub const BLOCK_TXS_CEILING: u64 = BLOCK_BYTES_CEILING / MIN_TX_ENTRY_BYTES;

/// The block transaction count a chain starts with: the 65,536 the decoder
/// allowed before the count was governed, so nothing moves at genesis.
pub const DEFAULT_BLOCK_TXS: u64 = 65_536;

const _: () = assert!(MAX_BLOCK_BYTES <= BLOCK_BYTES_CEILING);
const _: () = assert!(DEFAULT_BLOCK_TXS <= BLOCK_TXS_CEILING);

/// The most of each base fee that may go to the treasury, in basis points.
///
/// The rest is burned. A treasury share of 100% would turn the burn — the only
/// thing that makes the fee a cost to the chain rather than a transfer — off.
pub const MAX_TREASURY_BPS: u64 = 5_000;

/// One hundred percent, in basis points.
pub const BPS: u64 = 10_000;
