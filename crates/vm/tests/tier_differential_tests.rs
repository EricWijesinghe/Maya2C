//! Differential gas tests across execution tiers (Master Prompt 5 §2:
//! "Gas must be IDENTICAL in every tier").
//!
//! Every module in the corpus runs through Cranelift (cold), Cranelift
//! (cached), and Pulley, with several gas limits including ones that run out
//! mid-call. Output, gas used, events, final storage and the class of any
//! error must match exactly.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::host::MemoryState;
use maya_vm::{Tier, Vm};

const CONTRACT: [u8; 32] = [9u8; 32];

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

/// Hand-written modules, each aimed at a different part of the machine.
fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "integer loop",
            wasm(
                r#"(module (memory (export "memory") 1)
                  (func (export "invoke") (param i32) (result i64)
                    (local $i i64) (local $acc i64)
                    (loop $l
                      (local.set $acc (i64.add (local.get $acc) (i64.mul (local.get $i) (local.get $i))))
                      (local.set $i (i64.add (local.get $i) (i64.const 1)))
                      (br_if $l (i64.lt_u (local.get $i) (i64.const 20000))))
                    (i64.const 0)))"#,
            ),
        ),
        (
            "storage and events",
            wasm(
                r#"(module
                  (import "env" "storage_write" (func $w (param i32 i32 i32 i32)))
                  (import "env" "emit_event" (func $e (param i32 i32 i32 i32)))
                  (memory (export "memory") 1)
                  (data (i32.const 0) "key0value-0topic")
                  (func (export "invoke") (param i32) (result i64)
                    (local $i i32)
                    (loop $l
                      (i32.store8 (i32.const 3) (i32.add (i32.const 48) (local.get $i)))
                      (call $w (i32.const 0) (i32.const 4) (i32.const 4) (i32.const 7))
                      (call $e (i32.const 11) (i32.const 5) (i32.const 4) (i32.const 7))
                      (local.set $i (i32.add (local.get $i) (i32.const 1)))
                      (br_if $l (i32.lt_u (local.get $i) (i32.const 8))))
                    (i64.const 0)))"#,
            ),
        ),
        (
            "memory grow and bulk ops",
            wasm(
                r#"(module (memory (export "memory") 1)
                  (func (export "invoke") (param i32) (result i64)
                    (drop (memory.grow (i32.const 3)))
                    (memory.fill (i32.const 0) (i32.const 7) (i32.const 200000))
                    (memory.copy (i32.const 100000) (i32.const 0) (i32.const 50000))
                    (i64.const 0)))"#,
            ),
        ),
        (
            "NaN canonicalisation",
            wasm(
                r#"(module (memory (export "memory") 1)
                  (func (export "invoke") (param i32) (result i64)
                    (i64.store (i32.const 0)
                      (i64.reinterpret_f64 (f64.div (f64.const 0) (f64.const 0))))
                    (i64.const 0)))"#,
            ),
        ),
        (
            "trap: divide by zero",
            wasm(
                r#"(module (memory (export "memory") 1)
                  (func (export "invoke") (param i32) (result i64)
                    (drop (i32.div_u (i32.const 1) (local.get 0)))
                    (i64.const 0)))"#,
            ),
        ),
        (
            "infinite loop",
            wasm(
                r#"(module (memory (export "memory") 1)
                  (func (export "invoke") (param i32) (result i64)
                    (loop $forever (br $forever)) (i64.const 0)))"#,
            ),
        ),
    ]
}

/// Everything observable about a call: the outcome (output, gas, events),
/// the error with only its tier-independent part, and every byte of final
/// storage and the event log.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    outcome: Result<maya_vm::Outcome, String>,
    storage: std::collections::BTreeMap<(maya_vm::ContractId, Vec<u8>), Vec<u8>>,
    events: Vec<maya_vm::Event>,
}

/// The error without the parts that are formatting, not behaviour. A wasm
/// trap's text carries a backtrace and a "N: " cause index (how deep the tier
/// wrapped it, fuzz-found 2026-11-14); the line from `wasm trap:` on is the
/// trap code.
fn error_class(error: &maya_vm::VmError) -> String {
    match error {
        maya_vm::VmError::Trap(text) => text
            .find("wasm trap:")
            .map_or(text.as_str(), |at| text[at..].lines().next().unwrap_or(""))
            .to_string(),
        other => format!("{other:?}"),
    }
}

/// The one harness every test here runs a call through.
fn observe(
    vm: &Vm,
    code: &[u8],
    input: &[u8],
    gas: u64,
    state: MemoryState,
) -> (Observed, MemoryState) {
    let exec = vm.execute(code, CONTRACT, input, gas, state);
    let observed = Observed {
        outcome: exec.outcome.map_err(|e| error_class(&e)),
        storage: exec.state.storage.clone(),
        events: exec.state.events.clone(),
    };
    (observed, exec.state)
}

fn vms() -> Vec<(&'static str, Vm)> {
    vec![
        (
            "cranelift",
            Vm::with_tier(Tier::Cranelift).expect("cranelift"),
        ),
        ("pulley", Vm::with_tier(Tier::Pulley).expect("pulley")),
    ]
}

// FINDING (2026-09-27), explained 2026-11-23: on Windows the Pulley tier
// charged a constant 61,440 more fuel than Cranelift. The cause was data
// segments, not Pulley: wasmtime's copy-on-write memory image (unavailable
// on Windows) is padded to the compiler's page size and copied by metered
// code, so Windows charged 4 KiB (Cranelift) or 64 KiB (Pulley) per image
// and Linux charged nothing. `deterministic_config` now turns the image off;
// see `a_data_segment_costs_the_same_gas_on_every_host_and_tier`. These tests
// were ignored on Windows until then and now run on every platform.
#[test]
fn every_corpus_module_costs_the_same_gas_in_every_tier() {
    let vms = vms();
    let mut lines = Vec::new();
    for (name, code) in corpus() {
        for gas in [5_000u64, 50_000, 500_000, 10_000_000] {
            let mut seen: Vec<(&str, Observed)> = Vec::new();
            for (tier, vm) in &vms {
                // Twice per tier: the second call is a module-cache hit.
                for pass in ["cold", "cached"] {
                    let (obs, _) = observe(vm, &code, &[], gas, MemoryState::at_height(7));
                    seen.push((if pass == "cold" { tier } else { "cached" }, obs));
                }
            }
            for (label, obs) in &seen[1..] {
                assert_eq!(
                    obs, &seen[0].1,
                    "{name} at gas {gas}: {label} differs from cranelift"
                );
            }
            lines.push(golden_line(&name.replace(' ', "_"), gas, &[], &seen[0].1));
        }
    }
    // Most fuzz-corpus cells trap, and a trap carries no gas, so this hand
    // corpus is where the golden pins gas used (its "storage and events"
    // module has a data segment).
    check_golden(HAND_GOLDEN, &lines);
}

#[test]
fn the_token_swap_contract_costs_the_same_gas_in_every_tier() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target-contracts/wasm32-unknown-unknown/release/token_swap.wasm");
    // Committed to the repository (`git ls-files target-contracts`), so CI
    // always has it: a missing file fails rather than passing vacuously.
    let code = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (scripts/build-contract.sh)", path.display()));
    let init = {
        let mut v = vec![0u8];
        v.extend_from_slice(&1_000_000u64.to_le_bytes());
        v.extend_from_slice(&2_000_000u64.to_le_bytes());
        v
    };
    let swap = |method: u8, amount: u64| {
        let mut v = vec![method];
        v.extend_from_slice(&amount.to_le_bytes());
        v
    };
    let script = [init, swap(1, 10_000), swap(2, 5_000), swap(1, 1), vec![3u8]];
    let mut per_tier = Vec::new();
    for (tier, vm) in vms() {
        let mut state = MemoryState::at_height(100);
        let mut trace = Vec::new();
        for call in &script {
            let (obs, next) = observe(&vm, &code, call, 10_000_000, state);
            trace.push(obs.outcome);
            state = next;
        }
        per_tier.push((tier, trace));
    }
    assert_eq!(
        per_tier[0].1, per_tier[1].1,
        "token swap diverges between tiers"
    );
    assert!(per_tier[0].1.iter().all(Result::is_ok));
}

/// The committed differential corpus: modules generated by the
/// `vm_tier_differential` fuzz target (`fuzz/`) and dumped with
/// `MAYA_VM_DUMP_WASM`, then selected by hash order under 4 KiB (not
/// minimised; reports/05-vm.md). CI replays them here without a module
/// generator.
fn fuzz_corpus() -> Vec<(String, Vec<u8>)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tier_corpus");
    let mut modules: Vec<_> = std::fs::read_dir(&dir)
        .expect("tests/tier_corpus exists")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&path).expect("read module"))
        })
        .collect();
    // Directory order is the filesystem's; the run order should not be.
    modules.sort();
    modules
}

/// The state the fuzz target starts from, plus one fresh oracle feed at id 0
/// (where an unwritten guest pointer lands), so the oracle reads take their
/// success path here too.
fn corpus_state() -> MemoryState {
    let mut state = MemoryState::at_height(7);
    state.caller = Some([3u8; 32]);
    state.randomness = Some([5u8; 32]);
    state.set_balance([3u8; 32], 1_000_000);
    state.feeds.insert(
        [0u8; 32],
        maya_vm::host::OracleValue {
            value: maya_vm::Amount::from_u64(123_456_789),
            updated_height: 5,
            round: 3,
            observations: 4,
        },
    );
    state
}

/// One line per (module, gas, input) cell: what every tier on every host
/// must produce. A tier comparison on one host cannot catch a divergence that
/// hits both tiers alike (the data-segment bug did exactly that on Linux), so
/// the values are pinned, not only compared.
const GOLDEN: &str = "tests/tier_corpus/golden.txt";
/// The same for the hand-written corpus in [`corpus`].
const HAND_GOLDEN: &str = "tests/tier_corpus/hand_golden.txt";
/// Set to regenerate the golden file instead of checking it. Only for a
/// deliberate gas-schedule or wasmtime change, cited in an ADR (ADR-034).
const BLESS: &str = "MAYA_VM_BLESS_TIER_GOLDEN";

/// `<module> <gas> <input len> <gas used, or -> <blake3 of the observation> <error, or ok>`.
fn golden_line(name: &str, gas: u64, input: &[u8], observed: &Observed) -> String {
    let module = name.get(..16).unwrap_or(name);
    let gas_used = observed
        .outcome
        .as_ref()
        .map_or_else(|_| "-".to_string(), |o| o.gas_used.to_string());
    let digest = blake3::hash(format!("{observed:?}").as_bytes()).to_hex();
    let class = observed.outcome.as_ref().err().map_or("ok", String::as_str);
    format!(
        "{module} {gas} {} {gas_used} {} {class}",
        input.len(),
        &digest[..32]
    )
}

#[test]
fn every_fuzz_corpus_module_behaves_identically_in_every_tier() {
    let corpus = fuzz_corpus();
    // An emptied directory must fail, not pass vacuously.
    assert!(corpus.len() >= 16, "only {} corpus modules", corpus.len());
    let vms = vms();
    let (_, cranelift) = &vms[0];
    let mut lines = Vec::new();
    for (name, code) in &corpus {
        for gas in [0u64, 1_000, 50_000, 2_000_000] {
            for input in [&[][..], &[7u8; 16][..]] {
                let run = |vm: &Vm| observe(vm, code, input, gas, corpus_state()).0;
                let cold = run(cranelift);
                let len = input.len();
                // The second call is a module-cache hit.
                assert_eq!(
                    run(cranelift),
                    cold,
                    "{name} at gas {gas}, input {len} bytes: cranelift cached differs from cold"
                );
                for (tier, vm) in &vms[1..] {
                    assert_eq!(
                        run(vm),
                        cold,
                        "{name} at gas {gas}, input {len} bytes: {tier} differs from cranelift"
                    );
                }
                lines.push(golden_line(name, gas, input, &cold));
            }
        }
    }
    check_golden(GOLDEN, &lines);
}

/// Compares `lines` with a committed golden file, or rewrites it under
/// `BLESS`.
fn check_golden(file: &str, lines: &[String]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    if std::env::var_os(BLESS).is_some() {
        std::fs::write(&path, lines.join("\n") + "\n").expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).expect("golden file exists");
    let expected: Vec<&str> = expected.lines().collect();
    for (got, want) in lines.iter().zip(&expected) {
        assert_eq!(got, want, "this host differs from {file}");
    }
    assert_eq!(lines.len(), expected.len(), "{file} cell count");
}

/// A committed module's name is the blake3 of its bytes, as the fuzz
/// target's dump writes it: a renamed or edited file is caught here.
#[test]
fn every_fuzz_corpus_file_is_named_by_its_hash() {
    for (name, code) in fuzz_corpus() {
        assert_eq!(name, format!("{}.wasm", blake3::hash(&code).to_hex()));
    }
}

/// A 16-byte active data segment and nothing else. Before 2026-11-23 its
/// gas depended on the host, not the module: wasmtime's copy-on-write memory
/// image let Linux skip the copy (2 gas), while Windows copied the whole
/// page-aligned image through the metered startup function (4,099 gas on
/// Cranelift's 4 KiB page, 65,539 on Pulley's 64 KiB). That was the "Pulley
/// +61,440 on Windows" finding, and it also split Cranelift on Windows from
/// Cranelift on Linux. Pinned as a number, not only compared across tiers,
/// because both tiers on one host agreeing is not enough.
#[test]
fn a_data_segment_costs_the_same_gas_on_every_host_and_tier() {
    let code = wasm(
        r#"(module (memory (export "memory") 1)
          (data (i32.const 0) "key0value-0topic")
          (func (export "invoke") (param i32) (result i64) (i64.const 0)))"#,
    );
    for (tier, vm) in vms() {
        let exec = vm.execute(&code, CONTRACT, &[], 1_000_000, MemoryState::at_height(7));
        let gas = exec.outcome.expect("runs").gas_used;
        assert_eq!(gas, 20, "{tier}: 16-byte data segment gas");
    }
}

/// Replays `spec/tests/vm_instantiation.json` (spec rule VMI-1) on every
/// tier. The WAT is for readers; the bytes are what the vector pins, so a
/// `wat` upgrade that encoded differently is caught here too.
#[test]
fn the_vm_instantiation_vectors_hold_in_every_tier() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/tests/vm_instantiation.json");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("vector file")).expect("json");
    let cases = doc["cases"].as_array().expect("cases");
    assert!(cases.len() >= 2, "vector file has {} cases", cases.len());
    let number = |v: &serde_json::Value| v.as_str().expect("decimal string").parse::<u64>();
    for case in cases {
        let id = case["id"].as_str().expect("id");
        let code = wasm(case["wat"].as_str().expect("wat"));
        let hex = code.iter().fold(String::new(), |mut hex, b| {
            use std::fmt::Write as _;
            write!(hex, "{b:02x}").expect("write to String");
            hex
        });
        assert_eq!(
            case["wasm_hex"].as_str(),
            Some(hex.as_str()),
            "{id}: wasm_hex"
        );
        let gas = number(&case["gas_limit"]).expect("gas_limit");
        let expect = &case["expect"];
        for (tier, vm) in vms() {
            let exec = vm.execute(&code, CONTRACT, &[], gas, MemoryState::at_height(7));
            match (expect["result"].as_str(), exec.outcome) {
                (Some("ok"), Ok(outcome)) => assert_eq!(
                    Ok(outcome.gas_used),
                    number(&expect["gas_used"]),
                    "{id} on {tier}: gas_used"
                ),
                (Some("error"), Err(error)) => assert_eq!(
                    format!("{error:?}").split([' ', '{', '(']).next(),
                    expect["error"].as_str(),
                    "{id} on {tier}: error"
                ),
                (want, got) => panic!("{id} on {tier}: expected {want:?}, got {got:?}"),
            }
            assert!(
                exec.state.storage.is_empty(),
                "{id} on {tier}: wrote storage"
            );
        }
    }
}
