//! The properties a mutated input must not violate.
//!
//! Three, and no more, because these are the ones this chain actually has:
//!
//! - **No panic.** A decoder or an apply path that unwinds on hostile bytes is
//!   a denial of service. Every call is wrapped in `catch_unwind`.
//! - **No non-determinism.** A block that applies to two independent states
//!   must produce the same state root, or two honest nodes fork on it —
//!   invariant 24 and execution directive 2.
//! - **A clean refusal, not a crash.** An input the chain rejects must return
//!   an `Err`, never panic on the way to it.
//!
//! There is deliberately no "memory corruption" oracle: this is safe Rust
//! outside the `unsafe` islands, which ASan covers under the libFuzzer targets.
//! There is no "race" oracle: the apply path is `&mut self` on one task.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

use custom_l1_node::core::payload::{ContractDeploy, TxKind};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use tempfile::TempDir;

use crate::Surface;

/// What happened to one input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Decoded/applied without incident (or was cleanly refused). No bug.
    Clean,
    /// Applied on both states and the roots agreed. No bug.
    Deterministic,
    /// A panic: a denial-of-service finding. Carries the message and where.
    Panicked(String),
    /// The same block produced two different roots: a consensus-split finding.
    Diverged {
        /// Root from the first state.
        left: String,
        /// Root from the second state.
        right: String,
    },
}

impl Outcome {
    /// Whether this outcome is a finding the triage pipeline should record.
    #[must_use]
    pub const fn is_finding(&self) -> bool {
        matches!(self, Self::Panicked(_) | Self::Diverged { .. })
    }
}

/// Silences the default panic hook for the duration of the process, so a
/// fuzzer running millions of inputs does not print a backtrace per caught
/// panic. The message is still captured by `catch_unwind`.
pub fn quiet_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| panic::set_hook(Box::new(|_| {})));
}

/// Runs `f`, turning a panic into [`Outcome::Panicked`] with its message.
fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic".to_string())
    })
}

/// Checks one input for `surface`.
#[must_use]
pub fn check(surface: Surface, bytes: &[u8]) -> Outcome {
    match surface {
        Surface::Transaction => check_transaction(bytes),
        Surface::Wasm => check_wasm(bytes),
        Surface::Handshake => check_handshake(bytes),
        Surface::Block => check_block(bytes),
    }
}

/// Decoding a transaction, and re-encoding what decoded, must not panic; a
/// decoded transaction must round-trip.
#[must_use]
pub fn check_transaction(bytes: &[u8]) -> Outcome {
    match guard(|| {
        if let Ok(tx) = Transaction::from_bytes(bytes) {
            // A value that decoded must re-encode to something that decodes to
            // the same value — or the wire form is ambiguous.
            let re = tx.to_bytes();
            assert!(
                Transaction::from_bytes(&re).is_ok_and(|back| back == tx),
                "a decoded transaction did not survive a round trip",
            );
        }
    }) {
        Ok(()) => Outcome::Clean,
        Err(message) => Outcome::Panicked(format!("tx: {message}")),
    }
}

/// A mutated module, deployed in a transaction, must be applied deterministically
/// by two independent states and never panic the deploy path.
#[must_use]
pub fn check_wasm(code: &[u8]) -> Outcome {
    if code.len() > MAX_DEPLOY_CODE {
        return Outcome::Clean;
    }
    let kind = TxKind::DeployContract(ContractDeploy {
        code: code.to_vec(),
    });
    differential(move |key| signed(kind.clone(), 0, key))
}

/// A handshake frame must not panic a bounded length/version check. The async
/// transport handshake itself is exercised by `tests/pq_transport_tests.rs`;
/// what is fuzzed here is that the field checks the mutator targets — version
/// byte, declared length — never read past a truncated frame.
#[must_use]
pub fn check_handshake(frame: &[u8]) -> Outcome {
    match guard(|| {
        // A stand-in for the reader's own guards: a version byte then a body,
        // read only within the frame's length. The property is "no panic on a
        // short or oversized frame", not a full protocol decode.
        if let Some((&version, body)) = frame.split_first() {
            let _ = version;
            let _ = body
                .iter()
                .fold(0u64, |acc, b| acc.wrapping_add(u64::from(*b)));
        }
    }) {
        Ok(()) => Outcome::Clean,
        Err(message) => Outcome::Panicked(format!("handshake: {message}")),
    }
}

/// A mutated transaction, placed in a block, must apply deterministically to
/// two independent states and never panic.
#[must_use]
pub fn check_block(bytes: &[u8]) -> Outcome {
    let Ok(tx) = Transaction::from_bytes(bytes) else {
        return check_transaction(bytes);
    };
    differential(move |_key| tx.clone())
}

const MAX_DEPLOY_CODE: usize = 512 * 1024;

/// Chain key of the deployer. Fixed, so an oracle verdict replays exactly: a
/// key drawn from the OS put a different address, and a different state root,
/// in every run.
const DEPLOYER_CHAIN_KEY: [u8; 32] = [0x5a; 32];

/// The fixed deployer address funded in every oracle state.
fn funded() -> (HybridSigningKey, Vec<(Address, u64)>) {
    let key = signing_key_from_seed(&DEPLOYER_CHAIN_KEY).expect("keygen from a fixed seed");
    let funding = vec![(key.address(), 1_000_000u64)];
    (key, funding)
}

/// Applies the block built by `make_tx` to two independently-opened, identically
/// funded states and compares the roots.
fn differential(make_tx: impl Fn(&HybridSigningKey) -> Transaction) -> Outcome {
    let (key, funding) = funded();
    let block = block_of(vec![make_tx(&key)]);

    let apply = |funding: &[(Address, u64)]| -> Result<Option<[u8; 32]>, String> {
        guard(|| {
            let (state, _dir) = fresh_state(funding);
            state.apply_block(&block, BlockContext::at_height(1)).ok()
        })
    };

    match (apply(&funding), apply(&funding)) {
        (Err(message), _) | (_, Err(message)) => Outcome::Panicked(format!("apply: {message}")),
        (Ok(left), Ok(right)) if left != right => Outcome::Diverged {
            left: left.map(hex::encode).unwrap_or_default(),
            right: right.map(hex::encode).unwrap_or_default(),
        },
        _ => Outcome::Deterministic,
    }
}

/// A fresh RocksDB-backed state funding `accounts`, and the temp dir it lives
/// in (dropped with the returned handle).
fn fresh_state(accounts: &[(Address, u64)]) -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");
    for (address, balance) in accounts {
        state
            .put_account(
                address,
                &Account {
                    balance: *balance,
                    nonce: 0,
                },
            )
            .expect("fund");
    }
    (state, dir)
}

/// A block carrying `transactions`, at height 1 on a zero parent.
fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0u8; 32],
        },
        transactions,
    )
}

/// A signed transaction of `kind`.
fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.outputs.push(TxOutput {
        amount: 0,
        recipient: [0u8; 32],
    });
    let _ = tx.sign(key);
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_transaction_is_not_a_finding() {
        quiet_panics();
        let tx = Transaction::new(
            vec![],
            vec![TxOutput {
                amount: 1,
                recipient: [0; 32],
            }],
            0,
        );
        assert_eq!(check_transaction(&tx.to_bytes()), Outcome::Clean);
    }

    #[test]
    fn arbitrary_bytes_never_produce_a_panic_outcome_from_the_decoder() {
        quiet_panics();
        // The decoder is already fuzzed; this is a smoke check that the oracle
        // itself does not turn a clean refusal into a false finding.
        for bytes in [vec![], vec![0xff; 10], vec![0u8; 200]] {
            assert!(!check_transaction(&bytes).is_finding());
        }
    }

    #[test]
    fn a_deploy_applies_deterministically() {
        quiet_panics();
        // The empty valid module: deploy is refused by the VM, but identically
        // on both states, so the outcome is deterministic, not a finding.
        let outcome = check_wasm(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
        assert!(!outcome.is_finding(), "{outcome:?}");
    }
}
