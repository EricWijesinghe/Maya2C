//! The time-travel debugger on the real `contracts/nft-game` module: record a
//! mint, then step forward and back through it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use maya_vm::host::MemoryState;
use maya2c_cli::record;

const CONTRACT: [u8; 32] = [77; 32];
const ADMIN: [u8; 32] = [1; 32];
const ALICE: [u8; 32] = [2; 32];
const GAS: u64 = 10_000_000;

fn nft_game() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target-contracts/wasm32-unknown-unknown/release/nft_game.wasm");
    std::fs::read(&path).expect("the tracked nft_game.wasm")
}

/// State after `init` by the admin, ready for a mint.
fn initialised(code: &[u8]) -> MemoryState {
    let mut state = MemoryState::at_height(10);
    state.caller = Some(ADMIN);
    let init = [&[0u8][..], &ADMIN].concat();
    let (_, state) = record(code, CONTRACT, &init, GAS, state).unwrap();
    state
}

fn mint_input(id: u64, to: [u8; 32]) -> Vec<u8> {
    [&[1u8][..], &id.to_le_bytes(), &to].concat()
}

#[test]
fn a_mint_can_be_stepped_forward_and_back() {
    let code = nft_game();
    let (mut session, _) = record(
        &code,
        CONTRACT,
        &mint_input(7, ALICE),
        GAS,
        initialised(&code),
    )
    .unwrap();

    let listing = session.command("l").unwrap();
    assert!(listing.contains("ok"), "the mint succeeded: {listing}");
    assert!(
        listing.contains("caller"),
        "the owner check reads the signer: {listing}"
    );
    assert!(
        listing.contains("write"),
        "the mint writes storage: {listing}"
    );

    // Before the call, the token's storage is absent; after it, present.
    let before = session.command("s").unwrap();
    let steps = listing.lines().filter(|l| l.contains(" write ")).count();
    assert!(steps >= 1);
    let last = listing.lines().count() - 1;
    let after = session.command(&format!("g {last}")).unwrap();
    assert!(after.starts_with(&format!("[{last}/")), "{after}");
    let at_end = session.command("s").unwrap();
    assert_ne!(
        before, at_end,
        "storage differs between the first and last step"
    );

    // Backward is exact: back to 0 shows the initial storage again.
    session.command("g 0").unwrap();
    assert_eq!(session.command("s").unwrap(), before);
    assert_eq!(
        session.command("b").unwrap(),
        "[0/".to_string() + &last.to_string() + "] before the call"
    );
    assert!(session.command("q").is_none());
}

#[test]
fn a_refused_call_is_still_recorded_up_to_the_refusal() {
    let code = nft_game();
    let mut state = initialised(&code);
    state.caller = Some(ALICE); // not the admin: minting must be refused
    let (mut session, _) = record(&code, CONTRACT, &mint_input(8, ALICE), GAS, state).unwrap();
    let listing = session.command("l").unwrap();
    assert!(listing.contains("failed"), "{listing}");
    assert!(
        listing.contains("caller"),
        "the refusal followed a caller check: {listing}"
    );
    assert!(
        !listing.contains(" write "),
        "a refused mint writes nothing: {listing}"
    );
}
