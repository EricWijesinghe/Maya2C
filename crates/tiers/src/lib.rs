//! The build tiers, as cargo features.
//!
//! This crate contains no code on purpose. Its manifest *is* the artifact:
//! three features — `core`, `extended`, `frontier` — each of which turns on a
//! set of optional dependencies.
//!
//! ```text
//! cargo build -p maya-tiers                      # core, the default
//! cargo build -p maya-tiers --features extended
//! cargo build -p maya-tiers --features frontier
//! ```
//!
//! # What a tier is, and is not
//!
//! A tier decides what a build **compiles**. It never decides what a node
//! **executes**.
//!
//! The distinction is not pedantry. The foundation brief's `extended` tier
//! names "`DeFi`, RWA, identity, bridges" — which are [`maya_dex`], `maya_rwa`,
//! `maya_identity` and `maya_iso20022` — and every one of those writes
//! records that live under the state root (invariant 25). A cargo feature
//! that removed one would make two honest nodes running the same tagged
//! release compute *different state roots*: a chain split produced by a build
//! flag, described by nothing in the protocol and committed to by no block.
//!
//! So the node is whole in every tier. What the tiers add on top of it are
//! separate processes and separate artifacts — an explorer, a faucet, a
//! wallet, an SDK, a GPU miner — none of which can change a state root.
//!
//! Turning a *subsystem* off is a different mechanism entirely, and this tree
//! already has it: an activation height of `u64::MAX`, which every node
//! compiles identically and which a written decision moves.
//! `ZKML_ACTIVATION_HEIGHT`, `STATELESS_ACTIVATION_HEIGHT`,
//! `HTLC_L_ACTIVATION_HEIGHT`, `THREAT_INTEL_ACTIVATION_HEIGHT` and
//! `IOT_ACTIVATION_HEIGHT` are all set that way today.
//!
//! See `docs/adr/ADR-002-feature-tiers.md`.
//!
//! # The other half
//!
//! `[workspace] default-members` in the root manifest means a bare
//! `cargo build` at the repository root compiles `crates/node` and `xtask`
//! and nothing else. This crate is how you ask for *more* than that by name.
//!
//! [`maya_dex`]: https://docs.rs/maya-dex

#![no_std]

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    /// The tiers are a manifest, not code. What is worth asserting is that the
    /// feature a build selected is the feature that is on — a `--features
    /// frontier` build that silently produced `core` would be invisible
    /// otherwise.
    #[test]
    fn the_selected_tier_is_the_one_that_is_on() {
        // `core` is in `default`, so it is on unless someone passed
        // `--no-default-features`.
        #[cfg(feature = "core")]
        {
            // `extended` and `frontier` are supersets, declared that way in
            // the manifest. If that nesting were ever broken, one of these
            // would fire.
            #[cfg(feature = "extended")]
            assert!(cfg!(feature = "core"), "extended must imply core");
            #[cfg(feature = "frontier")]
            assert!(cfg!(feature = "extended"), "frontier must imply extended");
        }
        #[cfg(not(feature = "core"))]
        assert!(
            !cfg!(feature = "extended") && !cfg!(feature = "frontier"),
            "a tier above core is on while core is off"
        );
    }
}
