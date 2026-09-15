//! Coverage-guided, structure-aware differential fuzzing of Maya2C.
//!
//! Read `docs/offsec-sandbox.md` first: it records how the brief this was built
//! from maps onto a Rust, proof-of-work, WASM chain, and why the answers are a
//! separate workspace, no auto-patching, and "no crash across N inputs" rather
//! than "100% crash resistance".
//!
//! # Layout
//!
//! - [`mutate`]: structure-aware mutators for the three surfaces the brief
//!   names — transactions, WASM modules, handshakes. Each takes a seed and a
//!   deterministic RNG and returns bytes biased toward *just-valid* inputs, the
//!   ones that get past `from_bytes` and reach logic.
//! - [`oracle`]: the properties a mutated input must not violate — no panic, no
//!   unbounded allocation, and (for blocks) the same state root on re-execution.
//! - [`triage`]: minimize, classify and dedup a finding, then emit a
//!   regression-test stub in the shape of `tests/exploit_replays.rs`. It never
//!   writes a patch.
//!
//! # What runs where
//!
//! Nothing here runs in a node. The LibAFL engine (`bin/fuzz.rs`, `libafl`
//! feature) drives the mutators against the [`oracle`] and shares the corpora
//! under `fuzz/`. The in-tree gate `tests/fuzz_harness.rs` is a separate, small,
//! dependency-free replay — it does not use this crate.

pub mod mutate;
pub mod oracle;
pub mod runner;
pub mod triage;

#[cfg(any(feature = "libafl", feature = "z3"))]
pub mod engine;

use rand_chacha::ChaCha20Rng;

/// The fuzzer's RNG. Seeded, so a run and a finding both replay exactly.
pub type Rng = ChaCha20Rng;

/// The surfaces a mutator and the oracle understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// A `custom_l1_node::core::Transaction`.
    Transaction,
    /// A WebAssembly contract module for the VM.
    Wasm,
    /// A post-quantum transport or relay handshake frame.
    Handshake,
    /// A whole `custom_l1_node::core::Block`, for the differential apply oracle.
    Block,
}

impl Surface {
    /// The surface named by `name`, for a CLI.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "tx" | "transaction" => Self::Transaction,
            "wasm" => Self::Wasm,
            "handshake" => Self::Handshake,
            "block" => Self::Block,
            _ => return None,
        })
    }

    /// The short name used on the command line and in corpus paths.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Transaction => "tx",
            Self::Wasm => "wasm",
            Self::Handshake => "handshake",
            Self::Block => "block",
        }
    }

    /// Every surface, for iterating.
    pub const ALL: [Self; 4] = [
        Self::Transaction,
        Self::Wasm,
        Self::Handshake,
        Self::Block,
    ];
}
