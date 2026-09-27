//! Multi-VM Phase A (Master Prompt 5 §3): both foreign engines run real code
//! against the unified account mapping, and their costs convert to fuel by
//! ratios that stay near what the engines actually cost.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_multivm::evm::EvmEngine;
use maya_multivm::gas::{FUEL_PER_EVM_GAS, FUEL_PER_SBF_CU};
use maya_multivm::sbf;
use maya_multivm::state::evm_address;

const ALICE: [u8; 32] = [0xA1; 32];
const BOB: [u8; 32] = [0xB0; 32];

/// Counter: each call increments slot 0 and returns the new value.
/// runtime = SLOAD 0, +1, SSTORE 0, return it (18 bytes); init copies it out.
fn counter_init_code() -> Vec<u8> {
    let runtime = hex::decode(concat!("600054600101806000556000526020", "6000f3")).unwrap();
    let mut init = hex::decode("6012600c60003960126000f3").unwrap();
    init.extend(runtime);
    init
}

#[test]
fn an_evm_contract_deployed_by_a_maya_account_keeps_state_between_calls() {
    let mut evm = EvmEngine::new();
    evm.fund(&ALICE, 1_000_000);
    let deployed = evm.deploy(&ALICE, counter_init_code(), 1_000_000).unwrap();
    let contract: [u8; 20] = deployed.output.as_slice().try_into().unwrap();
    let first = evm.call(&ALICE, contract, vec![], 0, 100_000).unwrap();
    let second = evm.call(&BOB, contract, vec![], 0, 100_000).unwrap();
    assert_eq!(first.output[31], 1);
    assert_eq!(
        second.output[31], 2,
        "storage persisted across calls and callers"
    );
    assert_eq!(first.fuel, first.gas_used * FUEL_PER_EVM_GAS);
    println!(
        "evm counter: deploy {} gas, call {} gas = {} fuel",
        deployed.gas_used, first.gas_used, first.fuel
    );
}

#[test]
fn value_moves_between_maya_accounts_through_their_evm_addresses() {
    let mut evm = EvmEngine::new();
    evm.fund(&ALICE, 1_000_000);
    let out = evm
        .call(&ALICE, evm_address(&BOB), vec![], 250, 100_000)
        .unwrap();
    assert_eq!(out.gas_used, 21_000);
    assert_eq!(evm.balance(&BOB), revm::primitives::U256::from(250));
    assert_eq!(
        evm.balance(&ALICE),
        revm::primitives::U256::from(1_000_000 - 250)
    );
}

#[test]
fn an_evm_revert_is_an_error_not_a_state_change() {
    let mut evm = EvmEngine::new();
    evm.fund(&ALICE, 1_000_000);
    // PUSH1 0 PUSH1 0 REVERT as runtime.
    let init = hex::decode(concat!("6005600c60003960056000f3", "60006000fd")).unwrap();
    let c: [u8; 20] = evm
        .deploy(&ALICE, init, 1_000_000)
        .unwrap()
        .output
        .try_into()
        .unwrap();
    assert!(evm.call(&ALICE, c, vec![], 0, 100_000).is_err());
}

#[test]
fn an_sbf_program_reads_the_mapped_account_and_is_metered() {
    let out = sbf::run(
        "add64 r10, 0\nldxdw r0, [r1+0]\nadd64 r0, 1\nexit",
        41,
        1_000,
    )
    .unwrap();
    assert_eq!(out.result, 42, "the program saw the account's balance");
    assert_eq!(out.fuel, out.compute_units * FUEL_PER_SBF_CU);
    println!(
        "sbf: {} compute units = {} fuel",
        out.compute_units, out.fuel
    );
}

#[test]
fn an_sbf_program_that_overruns_its_budget_or_its_memory_fails() {
    let spin = "add64 r10, 0\nmov64 r0, 0\nja -1\nexit";
    assert!(
        sbf::run(spin, 0, 1_000).is_err(),
        "an infinite loop ran past its budget"
    );
    let wild = "add64 r10, 0\nldxdw r0, [r1+64]\nexit";
    assert!(
        sbf::run(wild, 0, 1_000).is_err(),
        "read past the input region"
    );
}

/// Times one unit of each engine's cost on this machine and checks the fixed
/// conversion ratios stay within 2x of the measured cost relative to Maya VM
/// fuel. A ratio that drifted from reality fails here, not in production.
#[test]
#[cfg_attr(debug_assertions, ignore = "timing: meaningful only with --release")]
fn the_conversion_ratios_are_within_2x_of_measured_cost() {
    // Maya VM: a counted loop, fuel per second.
    let vm = maya_vm::runtime::Vm::new().unwrap();
    let wat = r#"(module (memory (export "memory") 1)
        (func (export "invoke") (param i32) (result i64) (local $i i32)
          (loop $l (local.set $i (i32.add (local.get $i) (i32.const 1)))
                   (br_if $l (i32.lt_u (local.get $i) (i32.const 2000000))))
          (i64.const 0)))"#;
    let code = wat::parse_str(wat).unwrap();
    // Warm-up first: the first call compiles the module, and the module
    // cache makes every later one skip that — timing the first measures the
    // compiler, not the fuel.
    let run_vm = || {
        let t = Instant::now();
        let e = vm.execute(
            &code,
            [9; 32],
            &[],
            1_000_000_000,
            maya_vm::host::MemoryState::default(),
        );
        t.elapsed().as_nanos() as f64 / e.outcome.unwrap().gas_used as f64
    };
    run_vm();
    let ns_per_fuel = (0..3).map(|_| run_vm()).fold(f64::INFINITY, f64::min);

    // EVM: a counted loop, gas per second.
    // i = 0; loop: i += 1; if 200000 > i jump loop; stop
    let loop_code = hex::decode("60005b6001018062030d401160025700").unwrap();
    let mut init = hex::decode(format!(
        "60{:02x}600c60003960{:02x}6000f3",
        loop_code.len(),
        loop_code.len()
    ))
    .unwrap();
    init.extend(&loop_code);
    let mut evm = EvmEngine::new();
    evm.fund(&ALICE, 1_000_000_000);
    let c: [u8; 20] = evm
        .deploy(&ALICE, init, 1_000_000)
        .unwrap()
        .output
        .try_into()
        .unwrap();
    let ns_per_gas = (0..3)
        .map(|_| {
            let t = Instant::now();
            let gas = evm.call(&ALICE, c, vec![], 0, 16_000_000).unwrap().gas_used;
            t.elapsed().as_nanos() as f64 / gas as f64
        })
        .fold(f64::INFINITY, f64::min);

    // SBF: a counted loop, compute units per second.
    let t = Instant::now();
    let prog = "add64 r10, 0\nmov64 r0, 0\nadd64 r0, 1\njlt r0, 2000000, -2\nexit";
    let cu = sbf::run(prog, 0, 100_000_000).unwrap().compute_units;
    let ns_per_cu = t.elapsed().as_nanos() as f64 / cu as f64;

    let measured_evm = ns_per_gas / ns_per_fuel;
    let measured_sbf = ns_per_cu / ns_per_fuel;
    println!(
        "measured: {ns_per_fuel:.2} ns/fuel, {ns_per_gas:.2} ns/EVM gas, {ns_per_cu:.2} ns/SBF cu \
         => fuel per gas {measured_evm:.2} (constant {FUEL_PER_EVM_GAS}), fuel per cu {measured_sbf:.2} (constant {FUEL_PER_SBF_CU})"
    );
    for (name, constant, measured) in [
        ("evm", FUEL_PER_EVM_GAS as f64, measured_evm),
        ("sbf", FUEL_PER_SBF_CU as f64, measured_sbf),
    ] {
        assert!(
            constant / measured <= 2.0 && measured / constant <= 2.0,
            "{name}: constant {constant} is more than 2x from the measured {measured:.2}"
        );
    }
}
