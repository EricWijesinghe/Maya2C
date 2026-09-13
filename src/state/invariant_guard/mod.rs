//! The invariant guard: what has to be true of a block before it commits, and
//! what happens when it is not.
//!
//! ## Where it runs
//!
//! [`StateDB::stage_block`] is the single place a block's overlay is produced.
//! Every commit path funnels through it — `apply_block`,
//! `apply_block_journaled`, `apply_block_checked`, and `preview_root` — so one
//! hook at the end of it covers all four. `preview_root` is the one that makes
//! this more than a check: a miner assembling a candidate through
//! [`Chain::candidate_block`](crate::consensus::chain::Chain::candidate_block)
//! runs the same guard, so it refuses to build a block that breaks an invariant
//! rather than minting one the network then rejects.
//!
//! ## The two kinds of check, and why they differ
//!
//! | Kind | Example | Outcome |
//! |---|---|---|
//! | [Conservation](conservation) | a block creates a unit of an asset from nothing | the block is **invalid**; nothing commits |
//! | [Anomaly](anomaly) | a pool is suddenly worth less per share | the block commits and the module is **halted** for [`BREAKER_BLOCKS`](limits::BREAKER_BLOCKS) blocks |
//!
//! A conservation failure has no innocent reading, so it is refused exactly as
//! a wrong state root is refused. An anomaly does have one — a threshold is a
//! judgement about what is unusual, not about what is legal — so the block
//! stands and the affected module stops accepting new work while somebody
//! looks.
//!
//! ## What this guard does not claim to catch
//!
//! The brief that produced it names re-entrancy, integer overflow and
//! flash-loan manipulation. Two of those three attacks have no surface on this
//! chain and the third is not what the phrase usually means, which
//! `tests/exploit_replays.rs` demonstrates rather than asserts:
//!
//! - **Re-entrancy** needs a contract-to-contract call. The VM exposes nine
//!   host functions and none of them calls a contract or moves value, and
//!   `Vm::validate` refuses an unknown import at deploy.
//! - **Integer overflow** is already refused: `ledger-math` is checked
//!   arithmetic, Kani-verified, and every call site turns `None` into
//!   [`BalanceOverflow`](crate::error::NodeError::BalanceOverflow), which
//!   fails the whole block.
//! - **Flash loans** do not exist. `SwapRoute` is self-funded — no borrow, no
//!   callback — and `l2-flash` is a payment-channel network rather than a
//!   lending one.
//!
//! What the guard is actually for is the class underneath all three: value
//! created from nothing by *any* future bug, across transparent accounts, AMM
//! reserves, channel escrow, governance locks and the shielded pool. Each of
//! those five is checked today only by its own local rules, and a bug in one of
//! them is invisible to the other four.
//!
//! ## Invariant 28
//!
//! The guard reads only committed state and the block. No clock, no
//! configuration, no node-local value, ever. A breaker that trips on one node
//! and not another changes which transactions are valid on each, which is a
//! chain split — the same reason invariant 24's state-root check reads only
//! what execution produced.
//!
//! [`StateDB::stage_block`]: crate::state::db::StateDB

pub mod anomaly;
pub mod breaker;
pub mod conservation;
pub mod limits;

use crate::core::TxKind;
use crate::error::Result;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

pub use breaker::{BreakerRecord, Invariant, guard_key};

/// A part of the chain the breaker can halt independently.
///
/// Deliberately coarser than [`TxKind`]: an operator reading an alert wants to
/// know that "the DEX is halted", and a breaker per transaction kind would let
/// an attacker halt one narrow operation while leaving the rest of a broken
/// subsystem running.
///
/// There is no `Transfer` module, and that is the design rather than an
/// omission. [`TxKind::Transfer`] maps to no module at all, so a plain
/// peer-to-peer payment is ungated by construction and no future edit can
/// accidentally gate it — which is what "maintaining basic peer transfers"
/// has to mean if it is to survive somebody adding a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Module {
    /// Payment channels: open, close, dispute, penalty.
    Channels,
    /// Trading: assets, pools, liquidity, swaps, the order book.
    Dex,
    /// The shielded pool.
    Shielded,
    /// Price feeds and the randomness beacon.
    Oracle,
    /// Stake, proposals, votes.
    Governance,
    /// WASM contracts: deploy and call.
    Vm,
    /// The sealed mempool: envelopes and decryption shares.
    Sealed,
    /// Identity: DID registration, rotation, revocation, and the attestations
    /// and revocation bitmaps issuers publish.
    Identity,
}

/// Every module, in tag order. The guard iterates it so a new variant cannot be
/// added without appearing everywhere the set is enumerated.
pub const MODULES: &[Module] = &[
    Module::Channels,
    Module::Dex,
    Module::Shielded,
    Module::Oracle,
    Module::Governance,
    Module::Vm,
    Module::Sealed,
    Module::Identity,
];

impl Module {
    /// Wire tag. Part of the storage key, so these numbers are permanent.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Channels => 0,
            Self::Dex => 1,
            Self::Shielded => 2,
            Self::Oracle => 3,
            Self::Governance => 4,
            Self::Vm => 5,
            Self::Sealed => 6,
            Self::Identity => 7,
        }
    }

    /// The module a tag names.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Channels),
            1 => Some(Self::Dex),
            2 => Some(Self::Shielded),
            3 => Some(Self::Oracle),
            4 => Some(Self::Governance),
            5 => Some(Self::Vm),
            6 => Some(Self::Sealed),
            7 => Some(Self::Identity),
            _ => None,
        }
    }

    /// A short name for logs and errors.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Channels => "channels",
            Self::Dex => "dex",
            Self::Shielded => "shielded pool",
            Self::Oracle => "oracle",
            Self::Governance => "governance",
            Self::Vm => "wasm vm",
            Self::Sealed => "sealed mempool",
            Self::Identity => "identity",
        }
    }

    /// The module a payload belongs to, or `None` for a payload no breaker
    /// gates.
    ///
    /// Exhaustive on purpose — no wildcard arm — so adding a [`TxKind`] is a
    /// compile error here until somebody decides which module owns it.
    #[must_use]
    pub const fn of(kind: &TxKind) -> Option<Self> {
        match kind {
            // A transfer is the one thing that keeps working through every
            // breaker. It is also the one arm that does no work in
            // `apply_kind_for`, so this is a statement of what already is.
            TxKind::Transfer => None,

            TxKind::OpenChannel(_)
            | TxKind::CooperativeClose(_)
            | TxKind::DisputeClose(_)
            | TxKind::PenaltyClaim(_)
            | TxKind::SettleBatch(_)
            | TxKind::FinalizeDispute(_) => Some(Self::Channels),

            TxKind::RegisterAsset(_)
            | TxKind::TransferAsset(_)
            | TxKind::CreatePool(_)
            | TxKind::AddLiquidity(_)
            | TxKind::RemoveLiquidity(_)
            | TxKind::Swap(_)
            | TxKind::SwapRoute(_)
            | TxKind::PlaceOrder(_)
            | TxKind::CancelOrder(_) => Some(Self::Dex),

            TxKind::Shielded(_) => Some(Self::Shielded),

            TxKind::CreateFeed(_)
            | TxKind::SubmitFeed(_)
            | TxKind::RotateAuthorities(_)
            | TxKind::SubmitBeacon(_) => Some(Self::Oracle),

            TxKind::ClaimWork(_)
            | TxKind::LockStake(_)
            | TxKind::UnlockStake(_)
            | TxKind::Propose(_)
            | TxKind::CastVote(_)
            | TxKind::CancelProposal(_) => Some(Self::Governance),

            TxKind::DeployContract(_) | TxKind::CallContract(_) => Some(Self::Vm),

            TxKind::Seal(_) | TxKind::RevealShare(_) => Some(Self::Sealed),

            TxKind::RegisterDid(_)
            | TxKind::RotateDidKey(_)
            | TxKind::RevokeDid(_)
            | TxKind::AnchorAttestation(_)
            | TxKind::SetRevocationBit(_) => Some(Self::Identity),
        }
    }
}

impl StateDB {
    /// Runs the guard over a staged block.
    ///
    /// Conservation first: if the block is invalid there is no state to
    /// protect, and writing a breaker record for a block that never commits
    /// would be writing to an overlay that is about to be dropped.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvariantViolation`](crate::error::NodeError::InvariantViolation)
    /// for a block that creates or destroys value, or a read or decode failure
    /// from committed state.
    pub(crate) fn check_invariants(
        &self,
        overlay: &mut Overlay,
        context: BlockContext,
    ) -> Result<()> {
        self.check_value_conservation(overlay)?;
        self.check_anomalies(overlay, context.height)
    }
}
