//! Resource semantics and capability checks, enforced by the VM host
//! (Master Prompt 23 §1): real WASM contracts calling the `maya_res` imports,
//! with `maya_contract_safety::Runtime` deciding every move.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_contract_safety::{Capability, Fault, Id, RateLimit, Right, Runtime, State};
use maya_vm::error::VmError;
use maya_vm::host::{Address, ContractId, Event, HostState, MemoryState};
use maya_vm::resources::{RESOURCE_OP_FUEL, ResourceState};
use maya_vm::runtime::Vm;

const CONTRACT: ContractId = [7u8; 32];
/// The contract's id in the resource runtime.
const ME: Id = 1;
const ALICE: Id = 2;
const BOB: Id = 5;
const TOKEN: u32 = 1;
const GAS: u64 = 10_000_000;

struct Host {
    inner: MemoryState,
    rt: Runtime,
}

impl HostState for Host {
    fn balance_of(&self, address: &Address) -> u64 {
        self.inner.balance_of(address)
    }
    fn block_height(&self) -> u64 {
        self.inner.block_height()
    }
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.storage_get(contract, key)
    }
    fn storage_set(&mut self, contract: &ContractId, key: Vec<u8>, value: Vec<u8>) {
        self.inner.storage_set(contract, key, value);
    }
    fn emit(&mut self, event: Event) {
        self.inner.emit(event);
    }
}

impl ResourceState for Host {
    fn resources(&mut self) -> &mut Runtime {
        &mut self.rt
    }
    fn actor(&self) -> Id {
        ME
    }
}

/// A contract whose `invoke` runs `body` and returns nothing.
fn contract(body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
            (import "maya_res" "self" (func $self (result i32)))
            (import "maya_res" "balance" (func $balance (param i32 i32) (result i64)))
            (import "maya_res" "transfer" (func $transfer (param i32 i32 i32 i64)))
            (import "maya_res" "mint" (func $mint (param i32 i32 i64)))
            (import "maya_res" "burn" (func $burn (param i32 i32 i64)))
            (memory (export "memory") 1)
            (func (export "invoke") (param i32) (result i64)
              {body}
              (i64.const 0)))"#
    ))
    .expect("valid WAT")
}

/// `transfer(from, to, TOKEN, amount)` as WAT.
fn transfer(from: Id, to: Id, amount: i64) -> String {
    format!(
        "(call $transfer (i32.const {from}) (i32.const {to}) (i32.const {TOKEN}) (i64.const {amount}))"
    )
}

/// ME holds 1,000 and BOB holds 1,000; supply 2,000.
fn host() -> Host {
    let mut state = State::default();
    state.balances.insert((ME, TOKEN), 1_000);
    state.balances.insert((BOB, TOKEN), 1_000);
    state.supply.insert(TOKEN, 2_000);
    let mut rt = Runtime::default();
    rt.state = state;
    Host {
        inner: MemoryState::default(),
        rt,
    }
}

fn run(body: &str, h: Host) -> (Host, Result<u64, VmError>) {
    let vm = Vm::new().unwrap();
    let exec = vm.execute_with_resources(&contract(body), CONTRACT, &[], GAS, h);
    (exec.state, exec.outcome.map(|o| o.gas_used))
}

fn balances(h: &Host) -> (u128, u128, u128) {
    let s = &h.rt.state;
    (
        s.balance(ME, TOKEN),
        s.balance(ALICE, TOKEN),
        s.balance(BOB, TOKEN),
    )
}

#[test]
fn a_contract_moves_its_own_units_and_supply_is_conserved() {
    let (h, out) = run(&transfer(ME, ALICE, 100), host());
    assert!(out.is_ok(), "{out:?}");
    assert_eq!(balances(&h), (900, 100, 1_000));
    assert_eq!(h.rt.state.supply[&TOKEN], 2_000);
}

#[test]
fn another_holders_units_need_their_capability_and_only_up_to_the_allowance() {
    let (h, out) = run(&transfer(BOB, ALICE, 50), host());
    assert_eq!(out, Err(VmError::Resource(Fault::NoCapability)));
    assert_eq!(balances(&h), (1_000, 0, 1_000));

    let mut granted = host();
    granted.rt.grant(Capability {
        grantor: BOB,
        holder: ME,
        kind: TOKEN,
        right: Right::Move,
        remaining: 50,
    });
    let (h, out) = run(&transfer(BOB, ALICE, 50), granted);
    assert!(out.is_ok(), "{out:?}");
    assert_eq!(balances(&h), (1_000, 50, 950));
    // The allowance is spent; the same call again is refused.
    let (h, out) = run(&transfer(BOB, ALICE, 1), h);
    assert_eq!(out, Err(VmError::Resource(Fault::NoCapability)));
    assert_eq!(balances(&h), (1_000, 50, 950));
}

#[test]
fn minting_and_burning_need_the_kinds_capability() {
    let mint = format!("(call $mint (i32.const {TOKEN}) (i32.const {ALICE}) (i64.const 7))");
    let (h, out) = run(&mint, host());
    assert_eq!(out, Err(VmError::Resource(Fault::NoCapability)));
    assert_eq!(h.rt.state.supply[&TOKEN], 2_000);

    let mut h = h;
    for right in [Right::Mint, Right::Burn] {
        h.rt.grant(Capability {
            grantor: 0,
            holder: ME,
            kind: TOKEN,
            right,
            remaining: 10,
        });
    }
    let (h, out) = run(&mint, h);
    assert!(out.is_ok(), "{out:?}");
    assert_eq!((h.rt.state.supply[&TOKEN], balances(&h).1), (2_007, 7));
    let burn = format!("(call $burn (i32.const {TOKEN}) (i32.const {ALICE}) (i64.const 7))");
    let (h, out) = run(&burn, h);
    assert!(out.is_ok(), "{out:?}");
    assert_eq!((h.rt.state.supply[&TOKEN], balances(&h).1), (2_000, 0));
}

#[test]
fn any_failure_reverts_every_move_the_call_made() {
    let first = transfer(ME, ALICE, 10);
    let cases = [
        (
            format!("{first} {}", transfer(ME, ALICE, 5_000)),
            "insufficient",
        ),
        (format!("{first} (unreachable)"), "trap"),
        (format!("{first} (loop $l (br $l))"), "out of gas"),
        (
            format!("{first} {}", transfer(ME, ALICE, -1)),
            "negative amount",
        ),
    ];
    for (body, why) in cases {
        let (h, out) = run(&body, host());
        assert!(out.is_err(), "{why}");
        assert_eq!(
            balances(&h),
            (1_000, 0, 1_000),
            "{why}: the first transfer is undone"
        );
    }
    let (_, out) = run(&format!("{first} (loop $l (br $l))"), host());
    assert!(matches!(out, Err(VmError::OutOfGas { .. })));
}

#[test]
fn a_declared_invariant_is_checked_when_the_call_ends() {
    let mut h = host();
    h.rt.declare_invariant(ME, "reserve >= 500", |s: &State| {
        s.balance(ME, TOKEN) >= 500
    });
    let (h, out) = run(&transfer(ME, ALICE, 600), h);
    assert_eq!(
        out,
        Err(VmError::Resource(Fault::Invariant("reserve >= 500")))
    );
    assert_eq!(balances(&h), (1_000, 0, 1_000));
    let (h, out) = run(&transfer(ME, ALICE, 500), h);
    assert!(out.is_ok(), "{out:?}");
    assert_eq!(balances(&h), (500, 500, 1_000));
}

#[test]
fn an_outflow_over_the_limit_is_queued_not_executed() {
    let mut h = host();
    h.rt.limit_outflow(
        ME,
        RateLimit {
            kind: TOKEN,
            max: 100,
            window: 10,
        },
    );
    let (h, out) = run(&transfer(ME, ALICE, 150), h);
    assert_eq!(
        out,
        Err(VmError::Resource(Fault::RateLimited { pending: 0 }))
    );
    assert_eq!(balances(&h), (1_000, 0, 1_000));
    assert_eq!(
        h.rt.pending.len(),
        1,
        "the queued outflow survives the revert"
    );
    assert_eq!(h.rt.pending[0].amount, 150);
}

#[test]
fn each_value_moving_import_is_metered() {
    let (_, none) = run("", host());
    let (_, one) = run(&transfer(ME, ALICE, 1), host());
    assert!(one.unwrap() >= none.unwrap() + RESOURCE_OP_FUEL);
}

#[test]
fn the_consensus_surface_does_not_resolve_maya_res() {
    let vm = Vm::new().unwrap();
    let module = contract(&transfer(ME, ALICE, 1));
    assert!(vm.validate(&module).is_err(), "not deployable on chain");
    let exec = vm.execute(&module, CONTRACT, &[], GAS, MemoryState::default());
    assert!(
        matches!(
            exec.outcome,
            Err(VmError::UnresolvedImport(_) | VmError::Trap(_))
        ),
        "{:?}",
        exec.outcome
    );
}

#[test]
fn a_contract_reads_its_id_and_balances() {
    let body = format!(
        "(if (i32.ne (call $self) (i32.const {ME})) (then unreachable))
         (if (i64.ne (call $balance (i32.const {BOB}) (i32.const {TOKEN})) (i64.const 1000)) (then unreachable))"
    );
    let (_, out) = run(&body, host());
    assert!(out.is_ok(), "{out:?}");
}
