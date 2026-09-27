//! The host surface: exactly which imports a contract may name.
//!
//! [`HOST_FUNCTIONS`] is a hand-written list, and a hand-written list is a
//! second copy of what `register_host_functions` does. These tests are what
//! stop the two drifting: every name on the list must instantiate, and a name
//! that is not on it must be refused at *deploy*, before the module is stored.
//!
//! The deploy-time half is the one that matters for re-entrancy. Wasmtime
//! compiles a module without resolving its imports, so before this check a
//! module importing `env.call` was stored on chain and failed only when
//! somebody invoked it — turning a deployer's mistake into every caller's, and
//! leaving a contract on the chain that reads as if the call surface existed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::runtime::{HOST_FUNCTIONS, HOST_MODULE};
use maya_vm::{Vm, VmError};

/// A module importing one function and doing nothing with it.
fn module_importing(module: &str, name: &str, signature: &str) -> Vec<u8> {
    let text = format!(
        "(module
    (import \"{module}\" \"{name}\" (func $imported {signature}))
    (memory (export \"memory\") 1)
    (func (export \"invoke\") (param i32) (result i64) (i64.const 0)))"
    );
    wat::parse_str(&text).expect("valid WAT")
}

/// Signatures for the ten, in the same order as [`HOST_FUNCTIONS`].
const SIGNATURES: &[&str] = &[
    "(result i64)",
    // caller (ADR-026)
    "(param i32) (result i32)",
    "(param i32) (result i64)",
    "(param i32 i32 i32 i32) (result i32)",
    "(param i32 i32 i32 i32)",
    "(param i32)",
    "(param i32 i32) (result i32)",
    "(param i32) (result i64)",
    "(param i32 i32)",
    "(param i32 i32 i32 i32 i32 i32 i32) (result i32)",
];

#[test]
fn the_list_and_the_signatures_stay_the_same_length() {
    // The one thing a reader of the table above cannot check by eye.
    assert_eq!(HOST_FUNCTIONS.len(), SIGNATURES.len());
    assert_eq!(
        HOST_FUNCTIONS.len(),
        10,
        "the host surface changed size (caller: ADR-026)"
    );
}

/// Every `linker.func_wrap("env", "<name>", …)` in the crate's own source.
///
/// The other tests here check the list in one direction only — that everything
/// *on* it resolves — and a name the linker registers but the list omits
/// passes all of them. That is not hypothetical: `host_verify_zkml_proof` is
/// registered in `src/zkml.rs` rather than beside the other eight in
/// `src/runtime.rs`, was left off the list when `Vm::validate` started
/// enforcing it, and every zkML contract stopped being deployable while this
/// file stayed green. So the registrations are read from the source, which is
/// the only place both halves are written down.
fn registered_names() -> Vec<String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
        .expect("the crate's own src directory")
    {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source");
        for call in source.split("func_wrap(").skip(1) {
            let mut quoted = call.split('"').skip(1).step_by(2);
            let (Some(module), Some(name)) = (quoted.next(), quoted.next()) else {
                continue;
            };
            assert_eq!(
                module,
                HOST_MODULE,
                "{}: registers under module {module:?}, which no contract may import",
                path.display()
            );
            names.push(name.to_string());
        }
    }
    names.sort();
    names
}

#[test]
fn the_list_is_exactly_what_the_crate_registers() {
    let mut listed: Vec<String> = HOST_FUNCTIONS.iter().map(|s| (*s).to_string()).collect();
    listed.sort();
    let registered = registered_names();

    let missing: Vec<_> = registered.iter().filter(|n| !listed.contains(n)).collect();
    assert!(
        missing.is_empty(),
        "registered but not in HOST_FUNCTIONS, so no contract importing them can deploy: {missing:?}"
    );

    let extra: Vec<_> = listed.iter().filter(|n| !registered.contains(n)).collect();
    assert!(
        extra.is_empty(),
        "in HOST_FUNCTIONS but registered nowhere, so a contract passes deploy and traps on call: {extra:?}"
    );
}

#[test]
fn every_listed_host_function_validates() {
    let vm = Vm::new().expect("engine");
    for (name, signature) in HOST_FUNCTIONS.iter().zip(SIGNATURES) {
        let wasm = module_importing(HOST_MODULE, name, signature);
        vm.validate(&wasm)
            .unwrap_or_else(|error| panic!("{HOST_MODULE}.{name} should validate: {error}"));
    }
}

#[test]
fn an_import_the_host_does_not_provide_is_refused_at_validation() {
    let vm = Vm::new().expect("engine");
    for name in [
        "call",
        "call_contract",
        "delegate_call",
        "transfer",
        "send_value",
        "self_destruct",
        "block_heigth",
    ] {
        let wasm = module_importing(HOST_MODULE, name, "(param i32) (result i32)");
        assert!(
            matches!(vm.validate(&wasm), Err(VmError::UnresolvedImport(_))),
            "{HOST_MODULE}.{name} must not validate"
        );
    }
}

#[test]
fn a_listed_name_under_another_module_is_still_refused() {
    // The names are not the surface; the pair is. A module called `wasi` or
    // `host` offering a `storage_write` is a different function that happens to
    // share a name.
    let vm = Vm::new().expect("engine");
    for module in ["wasi_snapshot_preview1", "host", "", "Env"] {
        let wasm = module_importing(module, "storage_write", "(param i32 i32 i32 i32)");
        assert!(
            matches!(vm.validate(&wasm), Err(VmError::UnresolvedImport(_))),
            "{module}.storage_write must not validate"
        );
    }
}

#[test]
fn a_module_importing_nothing_validates() {
    let vm = Vm::new().expect("engine");
    let wasm = wat::parse_str(
        "(module
            (memory (export \"memory\") 1)
            (func (export \"invoke\") (param i32) (result i64) (i64.const 0)))",
    )
    .expect("valid WAT");
    vm.validate(&wasm).expect("no imports, nothing to resolve");
}
