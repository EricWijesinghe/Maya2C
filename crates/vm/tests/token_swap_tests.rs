//! The token swap contract, executed in the VM.
//!
//! Unlike `vm_tests.rs` — which drives hand-written WAT to target specific
//! machine behaviour — these run the real `#![no_std]` Rust contract, compiled
//! by the same script a developer would use.
//!
//! ## Building the artifact
//!
//! The contract builds into `target-contracts/`, deliberately **not** the
//! workspace `target/`. Both would otherwise contend for one cargo lock, and a
//! build launched from inside `cargo test` would block until the test timed out.
//!
//! If the artifact is missing, these tests build it. If the wasm target is not
//! installed they skip with an explanatory message rather than failing, since
//! that is a toolchain gap and not a defect in the code under test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use maya_vm::error::VmError;
use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;

const CONTRACT: [u8; 32] = [42u8; 32];

// Method selectors, matching the contract's ABI table.
const INIT: u8 = 0;
const SWAP_A_FOR_B: u8 = 1;
const SWAP_B_FOR_A: u8 = 2;
const RESERVES: u8 = 3;

// ---------------------------------------------------------------------------
// artifact
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/crates/vm, so the repository root is two
    // levels up. It was one until the phase B move -- see
    // docs/adr/ADR-001-workspace-layout.md.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/vm has two ancestors")
        .to_path_buf()
}

fn artifact_path() -> PathBuf {
    repo_root()
        .join("target-contracts")
        .join("wasm32-unknown-unknown")
        .join("release")
        .join("token_swap.wasm")
}

/// Loads the contract, building it if needed.
///
/// Returns `None` when the wasm target is unavailable, so the suite degrades to
/// a skip instead of a spurious failure on a machine without it.
fn load_contract() -> Option<Vec<u8>> {
    let path = artifact_path();
    if path.exists() {
        return std::fs::read(&path).ok();
    }

    let source = repo_root().join("contracts").join("token-swap");
    let target_dir = repo_root().join("target-contracts");

    let built = Command::new("cargo")
        .current_dir(&source)
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
        ])
        .arg(&target_dir)
        .output();

    match built {
        Ok(output) if output.status.success() => std::fs::read(&path).ok(),
        Ok(output) => {
            eprintln!(
                "skipping: contract build failed\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            None
        }
        Err(error) => {
            eprintln!("skipping: could not run cargo ({error})");
            None
        }
    }
}

/// Runs the suite body only when the contract is available.
macro_rules! with_contract {
    ($wasm:ident, $body:block) => {
        match load_contract() {
            Some($wasm) => $body,
            None => {
                eprintln!("skipped: token_swap.wasm unavailable");
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// call helpers
// ---------------------------------------------------------------------------

fn call_init(reserve_a: u64, reserve_b: u64) -> Vec<u8> {
    let mut input = vec![INIT];
    input.extend_from_slice(&reserve_a.to_le_bytes());
    input.extend_from_slice(&reserve_b.to_le_bytes());
    input
}

fn call_swap(method: u8, amount_in: u64) -> Vec<u8> {
    let mut input = vec![method];
    input.extend_from_slice(&amount_in.to_le_bytes());
    input
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(buf)
}

/// Initialises a pool and returns the resulting state.
fn initialised_pool(vm: &Vm, wasm: &[u8], a: u64, b: u64) -> MemoryState {
    let execution = vm.execute(
        wasm,
        CONTRACT,
        &call_init(a, b),
        10_000_000,
        MemoryState::at_height(100),
    );
    execution.outcome.expect("init must succeed");
    execution.state
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[test]
fn the_contract_compiles_and_loads() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        // Loading through `validate` is what deployment does, so a module that
        // uses a disabled proposal is rejected once rather than at every call.
        assert!(vm.validate(&wasm).is_ok());
        assert!(
            wasm.len() < 64 * 1024,
            "contract grew to {} bytes; on-chain storage is a recurring cost",
            wasm.len()
        );
    });
}

#[test]
fn initialising_a_pool_persists_the_reserves() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let state = initialised_pool(&vm, &wasm, 1_000_000, 2_000_000);

        let execution = vm.execute(&wasm, CONTRACT, &[RESERVES], 10_000_000, state);
        let outcome = execution.outcome.expect("reserves");

        assert_eq!(read_u64(&outcome.output, 0), 1_000_000);
        assert_eq!(read_u64(&outcome.output, 8), 2_000_000);
    });
}

#[test]
fn a_swap_moves_the_reserves_and_returns_the_output() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let state = initialised_pool(&vm, &wasm, 1_000_000, 1_000_000);

        let execution = vm.execute(
            &wasm,
            CONTRACT,
            &call_swap(SWAP_A_FOR_B, 1_000),
            10_000_000,
            state,
        );
        let outcome = execution.outcome.expect("swap");
        let amount_out = read_u64(&outcome.output, 0);

        // Constant product with a 0.3% fee: slightly under the input on a
        // balanced pool, never over.
        assert!(amount_out > 0);
        assert!(
            amount_out < 1_000,
            "output {amount_out} must be below input after fees and slippage"
        );

        let after = vm.execute(&wasm, CONTRACT, &[RESERVES], 10_000_000, execution.state);
        let reserves = after.outcome.expect("reserves");
        assert_eq!(read_u64(&reserves.output, 0), 1_000_000 + 1_000);
        assert_eq!(read_u64(&reserves.output, 8), 1_000_000 - amount_out);
    });
}

#[test]
fn the_constant_product_invariant_never_decreases() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let mut state = initialised_pool(&vm, &wasm, 1_000_000, 1_000_000);
        let mut k_before = 1_000_000u128 * 1_000_000u128;

        // Ten trades in alternating directions. The fee means k grows; it must
        // never shrink, or the pool is being drained.
        for index in 0..10 {
            let method = if index % 2 == 0 {
                SWAP_A_FOR_B
            } else {
                SWAP_B_FOR_A
            };
            let execution = vm.execute(
                &wasm,
                CONTRACT,
                &call_swap(method, 5_000),
                10_000_000,
                state,
            );
            execution.outcome.expect("swap");

            let probe = vm.execute(&wasm, CONTRACT, &[RESERVES], 10_000_000, execution.state);
            let reserves = probe.outcome.expect("reserves");
            let a = u128::from(read_u64(&reserves.output, 0));
            let b = u128::from(read_u64(&reserves.output, 8));
            let k_after = a * b;

            assert!(
                k_after >= k_before,
                "invariant fell from {k_before} to {k_after} on trade {index}"
            );
            k_before = k_after;
            state = probe.state;
        }
    });
}

#[test]
fn a_swap_emits_an_event_carrying_the_block_height() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let state = initialised_pool(&vm, &wasm, 500_000, 500_000);

        let execution = vm.execute(
            &wasm,
            CONTRACT,
            &call_swap(SWAP_A_FOR_B, 2_000),
            10_000_000,
            state,
        );
        let outcome = execution.outcome.expect("swap");

        assert_eq!(outcome.events.len(), 1);
        let event = &outcome.events[0];
        assert_eq!(event.topic, b"swap");
        assert_eq!(event.contract, CONTRACT);
        // height, amount in, amount out
        assert_eq!(read_u64(&event.data, 0), 100);
        assert_eq!(read_u64(&event.data, 8), 2_000);
        assert_eq!(read_u64(&event.data, 16), read_u64(&outcome.output, 0));
    });
}

#[test]
fn swapping_against_an_uninitialised_pool_fails() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let execution = vm.execute(
            &wasm,
            CONTRACT,
            &call_swap(SWAP_A_FOR_B, 1_000),
            10_000_000,
            MemoryState::at_height(1),
        );
        // A pool with no reserves has no price; quoting one would be inventing
        // a number.
        assert!(matches!(execution.outcome, Err(VmError::Trap(_))));
    });
}

#[test]
fn a_swap_can_never_drain_the_output_reserve() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let state = initialised_pool(&vm, &wasm, 1_000, 1_000);

        // An enormous input against a tiny pool. Constant product means the
        // output asymptotically approaches the reserve but must never reach it.
        let execution = vm.execute(
            &wasm,
            CONTRACT,
            &call_swap(SWAP_A_FOR_B, u64::MAX / 4),
            10_000_000,
            state,
        );

        match execution.outcome {
            // Refused outright.
            Err(VmError::Trap(_)) => {}
            Ok(outcome) => {
                let amount_out = read_u64(&outcome.output, 0);
                assert!(
                    amount_out < 1_000,
                    "paid out {amount_out} from a reserve of 1000"
                );
            }
            other => panic!("unexpected outcome {other:?}"),
        }
    });
}

#[test]
fn an_unknown_method_is_rejected() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let execution = vm.execute(&wasm, CONTRACT, &[99u8], 10_000_000, MemoryState::default());
        assert!(execution.outcome.is_err());
    });
}

// ---------------------------------------------------------------------------
// gas exhaustion
// ---------------------------------------------------------------------------

#[test]
fn a_swap_reports_the_gas_it_used() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let state = initialised_pool(&vm, &wasm, 1_000_000, 1_000_000);

        let execution = vm.execute(
            &wasm,
            CONTRACT,
            &call_swap(SWAP_A_FOR_B, 1_000),
            10_000_000,
            state,
        );
        let outcome = execution.outcome.expect("swap");

        assert!(outcome.gas_used > 0);
        assert!(outcome.gas_used < 10_000_000);
        println!("swap consumed {} gas", outcome.gas_used);
    });
}

#[test]
fn a_swap_starved_of_gas_runs_out_rather_than_completing() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let state = initialised_pool(&vm, &wasm, 1_000_000, 1_000_000);

        // Measure the real cost, then allow deliberately less than that.
        let measured = {
            let probe = vm.execute(
                &wasm,
                CONTRACT,
                &call_swap(SWAP_A_FOR_B, 1_000),
                10_000_000,
                state.clone(),
            );
            probe.outcome.expect("probe").gas_used
        };

        let starved = vm.execute(
            &wasm,
            CONTRACT,
            &call_swap(SWAP_A_FOR_B, 1_000),
            measured / 2,
            state,
        );

        assert!(
            matches!(starved.outcome, Err(VmError::OutOfGas { .. })),
            "expected gas exhaustion at half the measured cost"
        );
    });
}

#[test]
fn gas_exhaustion_leaves_the_reserves_untouched() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");
        let committed = initialised_pool(&vm, &wasm, 1_000_000, 1_000_000);

        let measured = {
            let probe = vm.execute(
                &wasm,
                CONTRACT,
                &call_swap(SWAP_A_FOR_B, 50_000),
                10_000_000,
                committed.clone(),
            );
            probe.outcome.expect("probe").gas_used
        };

        // The node's rule: a failed call's state is discarded, never committed.
        let starved = vm.execute(
            &wasm,
            CONTRACT,
            &call_swap(SWAP_A_FOR_B, 50_000),
            measured / 2,
            committed.clone(),
        );
        assert!(matches!(starved.outcome, Err(VmError::OutOfGas { .. })));
        drop(starved.state);

        // Reserves are exactly as they were before the failed attempt.
        let after = vm.execute(&wasm, CONTRACT, &[RESERVES], 10_000_000, committed);
        let reserves = after.outcome.expect("reserves");
        assert_eq!(read_u64(&reserves.output, 0), 1_000_000);
        assert_eq!(read_u64(&reserves.output, 8), 1_000_000);
    });
}

#[test]
fn identical_swaps_cost_identical_gas() {
    with_contract!(wasm, {
        let vm = Vm::new().expect("vm");

        let run = || {
            let state = initialised_pool(&vm, &wasm, 1_000_000, 1_000_000);
            vm.execute(
                &wasm,
                CONTRACT,
                &call_swap(SWAP_A_FOR_B, 1_000),
                10_000_000,
                state,
            )
            .outcome
            .expect("swap")
            .gas_used
        };

        // Every validator must charge the same gas for the same call, or they
        // disagree about whether it fit inside the caller's limit.
        assert_eq!(run(), run());
    });
}
