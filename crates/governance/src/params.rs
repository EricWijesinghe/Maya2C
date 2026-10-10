//! What a proposal is allowed to change, and to what.
//!
//! # The shape of self-amendment here
//!
//! Every governable rule is a `u64` under a [`ParameterKey`], stored in
//! consensus state, and read at execution time by whichever subsystem owns it.
//! A change carries an activation height, so the value that applies at height
//! *h* is a function of *h* alone — which is what makes two nodes replaying the
//! same chain reach the same answer.
//!
//! That covers two of the three shapes a rule change can take:
//!
//! | Shape | Example |
//! |---|---|
//! | A scalar limit or rate | the pool protocol fee, a per-block ceiling |
//! | A resource bound the VM enforces | contract memory pages, module size |
//!
//! The third — **flipping between two rules** — uses the same table. The binary
//! ships both implementations and a parameter decides which applies above a
//! height. `crypto::dag::registry` already works this way, and
//! `core::block::pow_hash_at` reads it: below the activation height the digest
//! is `ArgonBlake`, at or above it is the DAG. Governance moving such a height is
//! the same operation as governance moving a fee.
//!
//! # What is not here, and will not be
//!
//! **Native code.** There is no key whose value is a program. A node that
//! fetched executable code from chain state and ran it would be handing
//! arbitrary code execution on every machine in the network to whoever wins a
//! vote, and no timelock makes that acceptable.
//!
//! Substrate-style runtime upgrades work because a Substrate runtime *is*
//! sandboxed WebAssembly. This node's consensus logic is native Rust compiled
//! into the binary. Shipping the new logic and letting governance choose when it
//! activates is the honest version of the same idea, and it is what the table
//! above does.
//!
//! **The wasmtime engine.** `maya_vm::config` pins an exact version and says
//! why: the fuel schedule is not stable across releases, so two nodes on two
//! versions can disagree about whether a call ran out of gas. Governance can
//! move the VM's *limits*; moving its engine is a release.
//!
//! # Every value is checked twice
//!
//! Once when the proposal is made, and again when it executes. A release
//! between those two moments could tighten a range, and a value that was legal
//! when proposed must not become law after it stopped being legal.

use crate::error::{GovernanceError, Result};

/// A rule that governance may change.
///
/// The tags are consensus and must never be renumbered. A key this build does
/// not recognise is refused rather than ignored — see [`ParameterKey::from_tag`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ParameterKey {
    /// Basis points of every pooled swap taken by the protocol.
    ///
    /// Zero at genesis. `state::dex_exec::PROTOCOL_FEE_BPS` has said since it
    /// was written that nothing takes a cut "because there is no governance
    /// process that could have decided to". This is that process.
    DexProtocolFeeBps,
    /// Order fills one block may produce across every book.
    DexMaxFillsPerBlock,
    /// Resting orders one book may hold.
    DexMaxOrdersPerBook,
    /// Feed submissions one block may carry.
    OracleMaxFeedSubmissionsPerBlock,
    /// Shielded joinsplits one block may carry.
    ShieldedMaxPerBlock,
    /// Linear memory pages a contract may hold.
    VmMaxMemoryPages,
    /// Largest contract module a deployment may carry.
    VmMaxModuleBytes,
    /// Gas ceiling for a call that does not name one.
    VmDefaultGasLimit,
    /// Stake a proposer must lock alongside a proposal.
    GovernanceProposalDeposit,
    /// The signature suite new accounts are created under (ADR-007), as its
    /// registry byte. Read only once suite-tagged transactions activate.
    DefaultSignatureSuite,
}

/// Every key, in tag order.
///
/// Used to enumerate the table at genesis and to check that no key lacks a
/// bound.
pub const ALL_KEYS: &[ParameterKey] = &[
    ParameterKey::DexProtocolFeeBps,
    ParameterKey::DexMaxFillsPerBlock,
    ParameterKey::DexMaxOrdersPerBook,
    ParameterKey::OracleMaxFeedSubmissionsPerBlock,
    ParameterKey::ShieldedMaxPerBlock,
    ParameterKey::VmMaxMemoryPages,
    ParameterKey::VmMaxModuleBytes,
    ParameterKey::VmDefaultGasLimit,
    ParameterKey::GovernanceProposalDeposit,
    ParameterKey::DefaultSignatureSuite,
];

/// The permitted range and starting value of one parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// Smallest permissible value.
    pub min: u64,
    /// Largest permissible value.
    pub max: u64,
    /// Value in force before any proposal has changed it.
    pub default: u64,
}

impl Bounds {
    /// Whether `value` is within the range.
    #[must_use]
    pub const fn contains(&self, value: u64) -> bool {
        value >= self.min && value <= self.max
    }
}

impl ParameterKey {
    /// Stable wire tag. Never renumber — it is consensus.
    #[must_use]
    pub const fn tag(self) -> u16 {
        match self {
            Self::DexProtocolFeeBps => 1,
            Self::DexMaxFillsPerBlock => 2,
            Self::DexMaxOrdersPerBook => 3,
            Self::OracleMaxFeedSubmissionsPerBlock => 4,
            Self::ShieldedMaxPerBlock => 5,
            Self::VmMaxMemoryPages => 6,
            Self::VmMaxModuleBytes => 7,
            Self::VmDefaultGasLimit => 8,
            Self::GovernanceProposalDeposit => 9,
            Self::DefaultSignatureSuite => 10,
        }
    }

    /// Decodes a wire tag.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::UnknownParameter`] for a key this build does
    /// not implement.
    ///
    /// Refused, not ignored, and the distinction is the whole safety argument
    /// for a node meeting a chain that has moved past it. A node that skipped
    /// an unknown key would execute a proposal it did not understand and then
    /// continue building on a state every upgraded node disagrees with — a
    /// silent fork. Refusing turns that into a stopped node, which an operator
    /// can see and fix.
    pub const fn from_tag(tag: u16) -> Result<Self> {
        match tag {
            1 => Ok(Self::DexProtocolFeeBps),
            2 => Ok(Self::DexMaxFillsPerBlock),
            3 => Ok(Self::DexMaxOrdersPerBook),
            4 => Ok(Self::OracleMaxFeedSubmissionsPerBlock),
            5 => Ok(Self::ShieldedMaxPerBlock),
            6 => Ok(Self::VmMaxMemoryPages),
            7 => Ok(Self::VmMaxModuleBytes),
            8 => Ok(Self::VmDefaultGasLimit),
            9 => Ok(Self::GovernanceProposalDeposit),
            10 => Ok(Self::DefaultSignatureSuite),
            other => Err(GovernanceError::UnknownParameter { tag: other }),
        }
    }

    /// The range this parameter may be moved within, and where it starts.
    ///
    /// Every bound has a reason, and the reason is always the same shape: what
    /// breaks at each end. A range with no argument behind it is a range
    /// somebody will widen.
    #[must_use]
    pub const fn bounds(self) -> Bounds {
        match self {
            // Two percent, hard. The providers whose capital the pool uses are
            // paid from the LP rate; the protocol's cut comes out of the same
            // trade, so an unbounded one is an unbounded tax on every swap.
            Self::DexProtocolFeeBps => Bounds {
                min: 0,
                max: 200,
                default: 0,
            },
            // Below 64 the matcher cannot clear an ordinary crossed book in one
            // block and the backlog never drains; above 4096 one block's
            // matching pass is work every node redoes at roughly 0.75 µs a
            // fill, which is three milliseconds of validation for one block.
            Self::DexMaxFillsPerBlock => Bounds {
                min: 64,
                max: 4_096,
                default: 1_024,
            },
            // The count is what every node pays for in storage and in book
            // reconstruction, and escrow does not bound it.
            Self::DexMaxOrdersPerBook => Bounds {
                min: 256,
                max: 65_536,
                default: 4_096,
            },
            // Each submission costs up to a quorum of post-quantum signature
            // verifications at ~286 µs apiece. Thirty-two full-quorum
            // submissions would be nearly two hundred milliseconds.
            Self::OracleMaxFeedSubmissionsPerBlock => Bounds {
                min: 1,
                max: 32,
                default: 8,
            },
            // A joinsplit carries a ~0.4 MB STARK. The ceiling is what an
            // 8 MiB gossip frame holds beside the rest of a block, and zero
            // would silently disable the shielded pool rather than removing it.
            // Not yet read by the node: `MAX_SHIELDED_PER_BLOCK` enforces the
            // cap directly, and these bounds only keep a proposal from
            // recording a value the node would not honour.
            Self::ShieldedMaxPerBlock => Bounds {
                min: 1,
                max: 16,
                default: 16,
            },
            // 1 MiB to 64 MiB of contract memory. The upper end is what a node
            // must be able to allocate for one call; the lower end is what a
            // non-trivial contract needs to exist at all.
            Self::VmMaxMemoryPages => Bounds {
                min: 16,
                max: 1_024,
                default: 256,
            },
            // Code is stored on chain verbatim and every node keeps it forever,
            // so this is a storage decision as much as a validation one.
            Self::VmMaxModuleBytes => Bounds {
                min: 64 * 1024,
                max: 2 * 1024 * 1024,
                default: 512 * 1024,
            },
            // A halting bound, not a price. Too low and no useful contract
            // completes; too high and one call can occupy a validator for
            // longer than a block.
            Self::VmDefaultGasLimit => Bounds {
                min: 100_000,
                max: 1_000_000_000,
                default: 10_000_000,
            },
            // Anti-spam, not a barrier. The deposit is returned unless the
            // proposal is withdrawn, so the ceiling exists to stop governance
            // pricing out its own participants.
            Self::GovernanceProposalDeposit => Bounds {
                min: 0,
                max: 1_000_000_000,
                default: 10_000,
            },
            // A registry byte, not a quantity: the range spans the
            // post-quantum ids (0x10 ML-DSA-65 .. 0x30 the hybrid) and so
            // excludes Ed25519 (0x01) by construction. A byte inside the range
            // that names no suite is refused by the node when it reads the
            // table (`crypto::suites::default_suite`), because this crate has
            // no dependencies and cannot know the registry. ML-DSA-87 (0x11)
            // is the brief's mainnet default.
            Self::DefaultSignatureSuite => Bounds {
                min: 0x10,
                max: 0x30,
                default: 0x11,
            },
        }
    }

    /// Short label for errors, logs, and explorers.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DexProtocolFeeBps => "dex.protocol_fee_bps",
            Self::DexMaxFillsPerBlock => "dex.max_fills_per_block",
            Self::DexMaxOrdersPerBook => "dex.max_orders_per_book",
            Self::OracleMaxFeedSubmissionsPerBlock => "oracle.max_feed_submissions_per_block",
            Self::ShieldedMaxPerBlock => "shielded.max_per_block",
            Self::VmMaxMemoryPages => "vm.max_memory_pages",
            Self::VmMaxModuleBytes => "vm.max_module_bytes",
            Self::VmDefaultGasLimit => "vm.default_gas_limit",
            Self::GovernanceProposalDeposit => "governance.proposal_deposit",
            Self::DefaultSignatureSuite => "crypto.default_signature_suite",
        }
    }

    /// Checks a proposed value against this parameter's range.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::ParameterOutOfRange`] with the range, so the
    /// rejection says what would have been acceptable rather than only that
    /// this was not.
    pub const fn check(self, value: u64) -> Result<()> {
        let bounds = self.bounds();
        if bounds.contains(value) {
            Ok(())
        } else {
            Err(GovernanceError::ParameterOutOfRange {
                key: self,
                value,
                min: bounds.min,
                max: bounds.max,
            })
        }
    }
}

/// One rule change carried by a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParameterChange {
    /// Which rule.
    pub key: ParameterKey,
    /// What it becomes.
    pub value: u64,
}

impl ParameterChange {
    /// Checks the change against its parameter's range.
    ///
    /// # Errors
    ///
    /// As [`ParameterKey::check`].
    pub const fn validate(&self) -> Result<()> {
        self.key.check(self.value)
    }
}

/// Most rule changes one proposal may carry.
///
/// Eight. A proposal is a single decision that voters either accept or reject
/// as a whole, so bundling is a way to carry an unpopular change on the back of
/// a popular one. Some bundling is legitimate — two limits that only make sense
/// together — and a bound is the difference between that and an omnibus.
pub const MAX_CHANGES_PER_PROPOSAL: usize = 8;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn every_key_round_trips_through_its_tag() {
        for key in ALL_KEYS {
            assert_eq!(ParameterKey::from_tag(key.tag()), Ok(*key));
        }
    }

    #[test]
    fn tags_are_unique_and_every_key_is_listed() {
        let mut tags: Vec<u16> = ALL_KEYS.iter().map(|key| key.tag()).collect();
        let count = tags.len();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), count, "two parameters share a tag");

        // A key absent from `ALL_KEYS` would be governable but invisible to
        // genesis, so the table would start without it and read a default that
        // exists in no one place.
        assert_eq!(count, 10);
    }

    #[test]
    fn an_unknown_tag_is_refused_rather_than_ignored() {
        // A node that skipped an unknown key would execute a proposal it did
        // not understand and then build on a state every upgraded node
        // disagrees with. Refusing turns a silent fork into a stopped node.
        for tag in [0u16, 11, 999, u16::MAX] {
            assert_eq!(
                ParameterKey::from_tag(tag),
                Err(GovernanceError::UnknownParameter { tag })
            );
        }
    }

    #[test]
    fn every_default_is_inside_its_own_range() {
        // A default outside its bounds would mean the chain starts in a state
        // no proposal could restore it to.
        for key in ALL_KEYS {
            let bounds = key.bounds();
            assert!(
                bounds.contains(bounds.default),
                "{} default {} is outside [{}, {}]",
                key.label(),
                bounds.default,
                bounds.min,
                bounds.max
            );
            assert!(
                bounds.min <= bounds.max,
                "{} has an inverted range",
                key.label()
            );
        }
    }

    #[test]
    fn a_value_outside_the_range_is_refused_at_both_ends() {
        let key = ParameterKey::DexProtocolFeeBps;
        let bounds = key.bounds();

        assert!(key.check(bounds.max).is_ok());
        assert!(key.check(bounds.min).is_ok());
        assert!(key.check(bounds.max + 1).is_err());

        // The rejection carries the range, so it says what would have been
        // acceptable rather than only that this was not.
        assert_eq!(
            key.check(u64::MAX),
            Err(GovernanceError::ParameterOutOfRange {
                key,
                value: u64::MAX,
                min: bounds.min,
                max: bounds.max,
            })
        );
    }

    #[test]
    fn the_protocol_fee_cannot_be_voted_above_two_percent() {
        // The providers whose capital the pool uses are paid from the LP rate;
        // the protocol's cut comes out of the same trade. An unbounded one is
        // an unbounded tax on every swap, and it is exactly the change a
        // captured governance would make.
        assert!(ParameterKey::DexProtocolFeeBps.check(200).is_ok());
        assert!(ParameterKey::DexProtocolFeeBps.check(201).is_err());
        assert!(ParameterKey::DexProtocolFeeBps.check(10_000).is_err());
    }

    #[test]
    fn no_parameter_can_be_voted_to_zero_where_zero_disables_a_subsystem() {
        // Setting a per-block ceiling to zero does not remove a subsystem, it
        // silently stops it while leaving every transaction that uses it
        // apparently valid.
        for key in [
            ParameterKey::OracleMaxFeedSubmissionsPerBlock,
            ParameterKey::ShieldedMaxPerBlock,
            ParameterKey::DexMaxFillsPerBlock,
            ParameterKey::DexMaxOrdersPerBook,
            ParameterKey::VmMaxMemoryPages,
        ] {
            assert!(key.check(0).is_err(), "{} may be set to zero", key.label());
        }
    }
}
