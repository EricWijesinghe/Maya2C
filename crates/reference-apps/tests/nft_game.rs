//! The NFT game contract (`contracts/nft-game`) in the real VM.
//!
//! Authorisation is the transaction's signer, from the `caller` host function
//! (ADR-026). Until that existed the contract took the acting account as an
//! argument and the last test here proved anyone could move anyone's token;
//! it now proves they cannot.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;

const CONTRACT: [u8; 32] = [77; 32];
const ADMIN: [u8; 32] = [1; 32];
const ALICE: [u8; 32] = [2; 32];
const MALLORY: [u8; 32] = [3; 32];
const GAS: u64 = 10_000_000;

fn wasm() -> Option<Vec<u8>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = root.join("target-contracts/wasm32-unknown-unknown/release/nft_game.wasm");
    if !out.exists() {
        let built = Command::new("cargo")
            .current_dir(root.join("contracts/nft-game"))
            .args([
                "build",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
                "--target-dir",
            ])
            .arg(root.join("target-contracts"))
            .status();
        if !matches!(built, Ok(s) if s.success()) {
            eprintln!("skipped: nft_game.wasm could not be built (wasm32 target missing?)");
            return None;
        }
    }
    std::fs::read(out).ok()
}

/// Calls the contract as a transaction signed by `signer`.
fn call(
    vm: &Vm,
    code: &[u8],
    mut state: MemoryState,
    signer: [u8; 32],
    input: &[u8],
) -> (MemoryState, Option<Vec<u8>>, u64) {
    state.caller = Some(signer);
    let e = vm.execute(code, CONTRACT, input, GAS, state);
    match e.outcome {
        Ok(o) => (e.state, Some(o.output), o.gas_used),
        Err(_) => (e.state, None, 0),
    }
}

fn args(method: u8, id: u64, to: Option<[u8; 32]>) -> Vec<u8> {
    let mut v = vec![method];
    v.extend_from_slice(&id.to_le_bytes());
    if let Some(to) = to {
        v.extend_from_slice(&to);
    }
    v
}

fn owner_of(vm: &Vm, code: &[u8], state: MemoryState, id: u64) -> (MemoryState, [u8; 32]) {
    let mut q = vec![3];
    q.extend_from_slice(&id.to_le_bytes());
    let (s, out, _) = call(vm, code, state, MALLORY, &q);
    (s, out.unwrap().try_into().unwrap())
}

fn minted(vm: &Vm, code: &[u8]) -> MemoryState {
    let init = [&[0u8][..], &ADMIN].concat();
    let (s, ok, _) = call(vm, code, MemoryState::at_height(10), ADMIN, &init);
    assert!(ok.is_some());
    let (s, ok, gas) = call(vm, code, s, ADMIN, &args(1, 7, Some(ALICE)));
    assert!(ok.is_some(), "the admin mints");
    println!("mint: {gas} gas; module {} bytes", code.len());
    s
}

#[test]
fn mint_transfer_and_level_up_follow_the_erc721_shape() {
    let Some(code) = wasm() else { return };
    let vm = Vm::new().unwrap();
    vm.validate(&code).unwrap();
    let s = minted(&vm, &code);
    let (s, owner) = owner_of(&vm, &code, s, 7);
    assert_eq!(owner, ALICE);
    let (s, again, _) = call(&vm, &code, s, ADMIN, &args(1, 7, Some(MALLORY)));
    assert!(again.is_none(), "a token id mints once");

    let (s, lvl, _) = call(&vm, &code, s, ALICE, &args(4, 7, None));
    assert_eq!(lvl.unwrap(), 1u64.to_le_bytes());
    let (s, twice, _) = call(&vm, &code, s, ALICE, &args(4, 7, None));
    assert!(twice.is_none(), "one level per block");

    let (s, moved, gas) = call(&vm, &code, s, ALICE, &args(2, 7, Some(MALLORY)));
    assert!(moved.is_some());
    println!("transfer: {gas} gas");
    let (_, owner) = owner_of(&vm, &code, s, 7);
    assert_eq!(owner, MALLORY);
}

/// The hole ADR-026 closed: a token moves only when its owner signs.
#[test]
fn only_the_owner_can_move_a_token() {
    let Some(code) = wasm() else { return };
    let vm = Vm::new().unwrap();
    let s = minted(&vm, &code);
    let (s, stolen, _) = call(&vm, &code, s, MALLORY, &args(2, 7, Some(MALLORY)));
    assert!(stolen.is_none(), "Mallory moved Alice's token");
    let (s, levelled, _) = call(&vm, &code, s, MALLORY, &args(4, 7, None));
    assert!(levelled.is_none(), "Mallory levelled Alice's token");
    let (s, minted_again, _) = call(&vm, &code, s, MALLORY, &args(1, 8, Some(MALLORY)));
    assert!(minted_again.is_none(), "a non-admin minted");
    let (_, owner) = owner_of(&vm, &code, s, 7);
    assert_eq!(owner, ALICE, "the token stayed with its owner");
}
