//! Red-team cases for the time-travel debugger, on the tracked
//! `contracts/nft-game` module: the recording must agree with what the call
//! really did, the cursor must survive any command, and a hostile or broken
//! module must end in a reported failure, never a panic.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use maya_vm::host::MemoryState;
use maya_vm::runtime::Vm;
use maya_vm::trace::Traced;
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
fn the_recordings_last_step_is_exactly_the_state_the_call_left() {
    let code = nft_game();
    let before = initialised(&code);
    let vm = Vm::new().unwrap();
    let exec = vm.execute_traced(
        &code,
        CONTRACT,
        &mint_input(7, ALICE),
        GAS,
        Traced::new(before.clone()),
    );
    assert!(exec.outcome.is_ok(), "{:?}", exec.outcome.err());
    let (after, timeline) = exec.state.finish();
    let end = timeline.storage_after(timeline.steps().len());
    assert!(!end.is_empty(), "a mint touches storage");
    for (key, shown) in &end {
        assert_eq!(
            shown.as_ref(),
            after.storage.get(key),
            "the debugger's final view of {key:?} is the real post-state"
        );
    }
    // And step 0 is the state before the call, for every key it touched.
    for (key, shown) in timeline.storage_after(0) {
        assert_eq!(shown.as_ref(), before.storage.get(&key), "{key:?} before");
    }
}

#[test]
fn stepping_forward_k_times_is_goto_k_and_back_undoes_next() {
    let code = nft_game();
    let (mut s, _) = record(
        &code,
        CONTRACT,
        &mint_input(7, ALICE),
        GAS,
        initialised(&code),
    )
    .unwrap();
    let total = s.total();
    assert!(total > 1, "the mint records several steps");
    let mut by_next = vec![(s.position(), s.storage_pairs(), s.event_pairs())];
    for _ in 0..total {
        s.command("n").unwrap();
        by_next.push((s.position(), s.storage_pairs(), s.event_pairs()));
    }
    for (k, seen) in by_next.iter().enumerate() {
        s.command(&format!("g {k}")).unwrap();
        assert_eq!(
            &(s.position(), s.storage_pairs(), s.event_pairs()),
            seen,
            "step {k}"
        );
    }
    // From the end, back k times retraces the same states in reverse.
    for k in (0..total).rev() {
        s.command("b").unwrap();
        assert_eq!(s.cursor(), k);
        assert_eq!(s.storage_pairs(), by_next[k].1, "back to {k}");
    }
}

#[test]
fn no_command_moves_the_cursor_out_of_the_recording() {
    let code = nft_game();
    let (mut s, _) = record(
        &code,
        CONTRACT,
        &mint_input(7, ALICE),
        GAS,
        initialised(&code),
    )
    .unwrap();
    let total = s.total();
    // Back at the start stays at the start.
    s.command("b").unwrap();
    assert_eq!(s.cursor(), 0);
    s.command("g 2").unwrap();
    let past_end = format!("g {}", total + 1);
    for bad in [
        "g",
        "g -1",
        "g x",
        "g 1.5",
        past_end.as_str(),
        "g 99999999999999999999999999",
        "goto",
        "nn",
        "\u{0}",
        "  ",
    ] {
        let reply = s.command(bad).expect("only q quits");
        assert_eq!(s.cursor(), 2, "`{bad}` moved the cursor: {reply}");
    }
    // Forward past the end clamps to the end.
    for _ in 0..total + 5 {
        s.command("next").unwrap();
    }
    assert_eq!(s.cursor(), total);
    s.goto(usize::MAX);
    assert_eq!(s.cursor(), total, "goto clamps");
    assert!(s.command("quit").is_none());
}

#[test]
fn recording_the_same_call_twice_gives_the_same_timeline() {
    let code = nft_game();
    let (mut a, sa) = record(
        &code,
        CONTRACT,
        &mint_input(9, ALICE),
        GAS,
        initialised(&code),
    )
    .unwrap();
    let (mut b, sb) = record(
        &code,
        CONTRACT,
        &mint_input(9, ALICE),
        GAS,
        initialised(&code),
    )
    .unwrap();
    assert_eq!(a.listing(), b.listing());
    assert_eq!(a.summary(), b.summary(), "same gas and outcome");
    assert_eq!(sa.storage, sb.storage);
    assert_eq!(a.command("l"), b.command("l"));
}

#[test]
fn a_call_that_runs_out_of_gas_is_recorded_as_failed() {
    let code = nft_game();
    for gas in [0, 1, 1_000] {
        let (mut s, _) = record(
            &code,
            CONTRACT,
            &mint_input(7, ALICE),
            gas,
            initialised(&code),
        )
        .unwrap();
        let (used, outcome) = s.summary();
        assert_eq!(used, None, "gas {gas}: no gas_used for a failed call");
        assert!(outcome.starts_with("failed"), "gas {gas}: {outcome}");
        assert!(s.command("l").unwrap().contains("failed"));
    }
}

#[test]
fn a_module_that_is_not_wasm_is_a_reported_failure() {
    for junk in [&b""[..], b"\0asm", b"not wasm at all", &[0xFF; 64]] {
        match record(junk, CONTRACT, &[], GAS, MemoryState::at_height(1)) {
            Ok((mut s, _)) => {
                assert_eq!(s.total(), 0, "nothing ran");
                assert!(s.summary().1.starts_with("failed"), "{}", s.summary().1);
                assert_eq!(s.command("n").unwrap(), "[0/0] before the call");
            }
            Err(e) => assert!(!e.to_string().is_empty()),
        }
    }
}
