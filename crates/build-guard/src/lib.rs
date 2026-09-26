//! The production build guard (Master Prompt 11 §3, ADR-016).
//!
//! A feature-unification point with no code. Every crate in the workspace that
//! has a switch which must never reach mainnet forwards it to one of this
//! crate's features:
//!
//! | Feature here | Forwarded from |
//! |---|---|
//! | `sim` | `maya-entropy/sim-sources` |
//! | `research` | `maya-crypto-pq/hqc` (draft, measured non-constant-time), `maya-custody-mpc/threshold-lattice` |
//! | `frontier` | `maya-tiers/frontier` |
//! | `test-util` | reserved for test-only fixtures a production binary must not carry |
//! | `insecure` | reserved for switches that weaken a check |
//!
//! and `custom-l1-node/production` forwards to `production`. Cargo unifies a
//! dependency's features across everything one build compiles, so if a
//! production binary's graph pulls in *any* of those switches, this crate is
//! compiled with both and the build stops here, before a binary exists.
//! `cargo xtask release-check` shows both runs: the clean build and the one
//! that is forced to fail.

#![no_std]

#[cfg(all(
    feature = "production",
    any(
        feature = "sim",
        feature = "research",
        feature = "frontier",
        feature = "test-util",
        feature = "insecure"
    )
))]
compile_error!(
    "a `production` build has a SIM, RESEARCH, frontier, test-util or insecure feature \
     enabled somewhere in its dependency graph; run `cargo tree -e features -i maya-build-guard` \
     to find which crate forwarded it (ADR-016)"
);

/// Whether this build is a production build.
pub const PRODUCTION: bool = cfg!(feature = "production");
