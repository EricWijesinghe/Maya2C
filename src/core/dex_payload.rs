//! Wire forms of the trading transactions.
//!
//! Split out of [`crate::core::payload`] because that module was already at the
//! size where finding anything meant scrolling. The tags themselves stay there:
//! they are one numbering, and one numbering belongs in one place.
//!
//! Every structure here is fixed-width apart from a route's leg list. Fixed
//! width means the decoder cannot be told to allocate, and it means a
//! transaction's size is a function of what it does rather than of who sent it.
//!
//! ## Two paths, priced differently
//!
//! [`SwapRequest`] does **not** execute where it appears in the block. It joins
//! that pair's batch and settles at the batch's single clearing price once
//! every transaction has been staged — see [`maya_dex::batch`] for why.
//!
//! [`SwapRoute`] does execute in place, leg after leg, because its legs depend
//! on each other and there is nothing to net a multi-hop path against. The
//! asymmetry is deliberate. A route is the arbitrage primitive: whoever sends
//! one is competing on speed and accepts that ordering matters. A plain swap is
//! the ordinary user's, and they are the ones being protected.
//!
//! It does not reopen the sandwich, because routes run *before* the block's
//! batches. A miner can put a trade in front of a victim's batch, but there is
//! no place left in the same block to unwind it, and a front-run that cannot be
//! unwound is just a position.

use maya_dex::types::PairId;

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::asset::{AssetId, SYMBOL_LEN};

/// Most hops one route may take.
///
/// Four. Enough for a triangular arbitrage and one more; every additional leg
/// is another pool a validator must load and rewrite, and another dimension in
/// the search space of routes a miner could grind through looking for one that
/// pays.
pub const MAX_ROUTE_LEGS: usize = 4;

/// Encoded size of one [`RouteLeg`].
pub const ROUTE_LEG_SIZE: usize = 32 + 1;

/// Registering a new asset.
///
/// The whole supply is credited to the sender. There is no mint operation and
/// no authority field — see [`crate::state::asset`] for why elastic supply is
/// left to contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetRegistration {
    /// Ticker, ASCII uppercase and digits, right-padded with zeros.
    pub symbol: [u8; SYMBOL_LEN],
    /// Units to create, all credited to the sender.
    pub total_supply: u64,
}

/// Moving a non-native asset.
///
/// The native coin moves through a transaction's ordinary outputs; routing it
/// through here as well would be a second spending path over the same balance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetTransfer {
    /// Asset to move.
    pub asset: AssetId,
    /// Account to credit.
    pub recipient: Address,
    /// Units to move.
    pub amount: u64,
}

/// Creating a pool and seeding it.
///
/// Creation and the first deposit are one transaction because a pool with no
/// reserves has no price, and a pool with no price is a slot in the state that
/// anybody can seed at any ratio they like — which is to say, at any price they
/// like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolCreation {
    /// One of the two assets. Ordering is canonicalised on execution, so the
    /// sender need not know which will be the base.
    pub asset_a: AssetId,
    /// The other.
    pub asset_b: AssetId,
    /// Basis points retained for liquidity providers.
    pub lp_fee_bps: u32,
    /// Units of `asset_a` to deposit.
    pub amount_a: u64,
    /// Units of `asset_b` to deposit.
    pub amount_b: u64,
}

/// Depositing into an existing pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiquidityDeposit {
    /// Pool to deposit into.
    pub pair: PairId,
    /// Most base the sender will supply.
    pub base_desired: u64,
    /// Most quote the sender will supply.
    pub quote_desired: u64,
    /// Fewest shares the sender will accept.
    ///
    /// The deposit is taken at the pool's ratio, which another transaction in
    /// the same block can have moved. This is the slippage bound for a deposit.
    pub min_shares: u64,
}

/// Redeeming shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiquidityWithdrawal {
    /// Pool to withdraw from.
    pub pair: PairId,
    /// Shares to burn.
    pub shares: u64,
    /// Least base the sender will accept.
    pub min_base: u64,
    /// Least quote the sender will accept.
    pub min_quote: u64,
}

/// A swap against a pool, settled in that pool's batch for the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapRequest {
    /// Pool to trade against.
    pub pair: PairId,
    /// Tag of a [`maya_dex::types::Direction`].
    pub direction: u8,
    /// Units supplied.
    pub amount_in: u64,
    /// Least the sender will accept in return.
    ///
    /// Missing it is **not** an error. The swap is skipped, the input is
    /// returned, and the transaction still occupies its nonce. Making it an
    /// error would let one trader's bound invalidate a block that carried it.
    pub min_out: u64,
    /// Last height at which the swap may execute. Zero means no deadline.
    ///
    /// Distinct from the slippage bound: a bound protects against the price
    /// moving, a deadline protects against the transaction sitting in a
    /// mempool for an hour and then executing against a world that has moved
    /// on. A miner who holds a transaction back cannot make it stale-execute.
    pub deadline: u64,
}

/// One hop of a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteLeg {
    /// Pool to trade against.
    pub pair: PairId,
    /// Tag of a [`maya_dex::types::Direction`].
    pub direction: u8,
}

/// An atomic multi-hop swap.
///
/// Every leg executes or none does. That is what makes it an arbitrage
/// instrument: a path that ends where it started with more than it began is
/// profitable, and a path that does not is rejected in full rather than
/// leaving the sender holding an intermediate asset they never wanted.
///
/// It is **not** an uncollateralised flash loan. The sender supplies
/// `amount_in` from their own balance; there is no borrow and no callback.
/// Lending the first leg against a repayment check would need re-entry into the
/// borrower's code, which means the contract host ABI, which is a larger change
/// than this one and is not pretended at here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapRoute {
    /// Hops, in order. The output asset of each must be the input of the next.
    pub legs: Vec<RouteLeg>,
    /// Units supplied to the first leg.
    pub amount_in: u64,
    /// Least the sender will accept out of the last leg.
    ///
    /// Missing it is a no-op, exactly as for a plain swap: nothing moves, no
    /// pool is rewritten, and the transaction keeps its nonce. Two
    /// arbitrageurs racing the same opportunity in one block is the *normal*
    /// case, and the loser's transaction must not take the block down with it.
    ///
    /// A malformed route is still an error — legs that do not join up, or a
    /// pool that does not exist. The distinction is between a transaction that
    /// is wrong and a transaction that merely lost.
    pub min_out: u64,
    /// Last height at which the route may execute. Zero means no deadline.
    pub deadline: u64,
}

/// Placing a limit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderPlacement {
    /// Book to rest in.
    pub pair: PairId,
    /// Tag of a [`maya_dex::book::Side`].
    pub side: u8,
    /// Limit price: quote units per [`maya_dex::types::PRICE_SCALE`] base
    /// units.
    pub price: u64,
    /// Base units to trade.
    pub amount: u64,
    /// Last height the order may match at. Zero is good-till-cancelled.
    pub expiry: u64,
}

impl AssetRegistration {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.symbol);
        buf.extend_from_slice(&self.total_supply.to_le_bytes());
    }

    /// Decodes a registration.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            symbol: reader.read_array::<SYMBOL_LEN>()?,
            total_supply: reader.read_u64()?,
        })
    }
}

impl AssetTransfer {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.asset);
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&self.amount.to_le_bytes());
    }

    /// Decodes a transfer.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            asset: reader.read_array::<32>()?,
            recipient: reader.read_array::<32>()?,
            amount: reader.read_u64()?,
        })
    }
}

impl PoolCreation {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.asset_a);
        buf.extend_from_slice(&self.asset_b);
        buf.extend_from_slice(&self.lp_fee_bps.to_le_bytes());
        buf.extend_from_slice(&self.amount_a.to_le_bytes());
        buf.extend_from_slice(&self.amount_b.to_le_bytes());
    }

    /// Decodes a pool creation.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            asset_a: reader.read_array::<32>()?,
            asset_b: reader.read_array::<32>()?,
            lp_fee_bps: reader.read_u32()?,
            amount_a: reader.read_u64()?,
            amount_b: reader.read_u64()?,
        })
    }
}

impl LiquidityDeposit {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.pair);
        buf.extend_from_slice(&self.base_desired.to_le_bytes());
        buf.extend_from_slice(&self.quote_desired.to_le_bytes());
        buf.extend_from_slice(&self.min_shares.to_le_bytes());
    }

    /// Decodes a deposit.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            pair: reader.read_array::<32>()?,
            base_desired: reader.read_u64()?,
            quote_desired: reader.read_u64()?,
            min_shares: reader.read_u64()?,
        })
    }
}

impl LiquidityWithdrawal {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.pair);
        buf.extend_from_slice(&self.shares.to_le_bytes());
        buf.extend_from_slice(&self.min_base.to_le_bytes());
        buf.extend_from_slice(&self.min_quote.to_le_bytes());
    }

    /// Decodes a withdrawal.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            pair: reader.read_array::<32>()?,
            shares: reader.read_u64()?,
            min_base: reader.read_u64()?,
            min_quote: reader.read_u64()?,
        })
    }
}

impl SwapRequest {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.pair);
        buf.push(self.direction);
        buf.extend_from_slice(&self.amount_in.to_le_bytes());
        buf.extend_from_slice(&self.min_out.to_le_bytes());
        buf.extend_from_slice(&self.deadline.to_le_bytes());
    }

    /// Decodes a swap.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            pair: reader.read_array::<32>()?,
            direction: reader.read_u8()?,
            amount_in: reader.read_u64()?,
            min_out: reader.read_u64()?,
            deadline: reader.read_u64()?,
        })
    }
}

impl SwapRoute {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&(self.legs.len() as u64).to_le_bytes());
        for leg in &self.legs {
            buf.extend_from_slice(&leg.pair);
            buf.push(leg.direction);
        }
        buf.extend_from_slice(&self.amount_in.to_le_bytes());
        buf.extend_from_slice(&self.min_out.to_le_bytes());
        buf.extend_from_slice(&self.deadline.to_le_bytes());
    }

    /// Decodes a route.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated, carries no
    /// legs, or carries more than [`MAX_ROUTE_LEGS`].
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let count = reader.read_collection_len(ROUTE_LEG_SIZE)?;
        if count == 0 {
            return Err(NodeError::Decode("route carries no legs".to_string()));
        }
        if count > MAX_ROUTE_LEGS {
            return Err(NodeError::Decode(format!(
                "route of {count} legs exceeds the maximum {MAX_ROUTE_LEGS}"
            )));
        }

        let mut legs = Vec::with_capacity(count);
        for _ in 0..count {
            legs.push(RouteLeg {
                pair: reader.read_array::<32>()?,
                direction: reader.read_u8()?,
            });
        }

        Ok(Self {
            legs,
            amount_in: reader.read_u64()?,
            min_out: reader.read_u64()?,
            deadline: reader.read_u64()?,
        })
    }
}

impl OrderPlacement {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.pair);
        buf.push(self.side);
        buf.extend_from_slice(&self.price.to_le_bytes());
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(&self.expiry.to_le_bytes());
    }

    /// Decodes a placement.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            pair: reader.read_array::<32>()?,
            side: reader.read_u8()?,
            price: reader.read_u64()?,
            amount: reader.read_u64()?,
            expiry: reader.read_u64()?,
        })
    }
}
