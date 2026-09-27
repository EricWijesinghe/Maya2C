//! Private state in a contract (Master Prompt 27 §1): a WASM contract keeps a
//! hidden balance as a commitment in its storage; the owner debits and
//! credits it with STARK proofs made off chain; the contract stores only
//! commitments and emits only public amounts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::error::VmError;
use maya_vm::host::{Address, ContractId, Event, HostState, MemoryState};
use maya_vm::private::{Direction, PrivateVerifier};
use maya_vm::runtime::Vm;
use maya_zk_stark::Proof;
use maya_zk_stark::hash::Digest;
use maya_zk_stark::private_state::{
    commit, from_bytes, prove_credit, prove_debit, to_bytes, verify_credit, verify_debit,
};

const CONTRACT: ContractId = [0x5E; 32];
const GAS: u64 = 1_000_000_000;

struct Host(MemoryState);

impl HostState for Host {
    fn balance_of(&self, a: &Address) -> u64 {
        self.0.balance_of(a)
    }
    fn block_height(&self) -> u64 {
        self.0.block_height()
    }
    fn storage_get(&self, c: &ContractId, k: &[u8]) -> Option<Vec<u8>> {
        self.0.storage_get(c, k)
    }
    fn storage_set(&mut self, c: &ContractId, k: Vec<u8>, v: Vec<u8>) {
        self.0.storage_set(c, k, v);
    }
    fn emit(&mut self, e: Event) {
        self.0.emit(e);
    }
}

impl PrivateVerifier for Host {
    fn verify_transition(
        &self,
        d: Direction,
        old: &[u8; 32],
        new: &[u8; 32],
        amount: u64,
        proof: &[u8],
    ) -> bool {
        let (Ok(old), Ok(new)) = (from_bytes(old), from_bytes(new)) else {
            return false;
        };
        let proof = Proof::from_bytes(proof.to_vec());
        match d {
            Direction::Debit => verify_debit(&proof, &old, &new, amount).is_ok(),
            Direction::Credit => verify_credit(&proof, &old, &new, amount).is_ok(),
        }
    }
}

/// `op(1) amount(8) new_commitment(32) proof(..)`; op 0 initialises.
fn vault() -> Vec<u8> {
    wat::parse_str(
        r#"(module
          (import "env" "storage_read" (func $read (param i32 i32 i32 i32) (result i32)))
          (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
          (import "env" "emit_event" (func $emit (param i32 i32 i32 i32)))
          (import "maya_priv" "verify_debit" (func $debit (param i32 i32 i64 i32 i32) (result i32)))
          (import "maya_priv" "verify_credit" (func $credit (param i32 i32 i64 i32 i32) (result i32)))
          (memory (export "memory") 100)
          (data (i32.const 0) "c")
          (data (i32.const 512) "priv")
          (func (export "input_ptr") (result i32) (i32.const 4096))
          (func (export "input_cap") (result i32) (i32.const 4194304))
          (func (export "invoke") (param $len i32) (result i64)
            (local $op i32) (local $ok i32)
            (local.set $op (i32.load8_u (i32.const 4096)))
            (if (i32.eqz (local.get $op))
              (then
                (if (i32.ge_s (call $read (i32.const 0) (i32.const 1) (i32.const 256) (i32.const 32)) (i32.const 0))
                  (then unreachable))
                (call $write (i32.const 0) (i32.const 1) (i32.const 4105) (i32.const 32))
                (return (i64.const 0))))
            (if (i32.ne (call $read (i32.const 0) (i32.const 1) (i32.const 256) (i32.const 32)) (i32.const 32))
              (then unreachable))
            (local.set $ok
              (if (result i32) (i32.eq (local.get $op) (i32.const 1))
                (then (call $debit (i32.const 256) (i32.const 4105) (i64.load (i32.const 4097))
                        (i32.const 4137) (i32.sub (local.get $len) (i32.const 41))))
                (else (call $credit (i32.const 256) (i32.const 4105) (i64.load (i32.const 4097))
                        (i32.const 4137) (i32.sub (local.get $len) (i32.const 41))))))
            (if (i32.eqz (local.get $ok)) (then unreachable))
            (call $write (i32.const 0) (i32.const 1) (i32.const 4105) (i32.const 32))
            (call $emit (i32.const 512) (i32.const 4) (i32.const 4096) (i32.const 9))
            (i64.const 0)))"#,
    )
    .unwrap()
}

fn input(op: u8, amount: u64, new: &Digest, proof: &[u8]) -> Vec<u8> {
    [&[op][..], &amount.to_le_bytes(), &to_bytes(new), proof].concat()
}

fn blind(tag: u8) -> Digest {
    maya_zk_stark::credential::digest_from_bytes(&[tag; 32])
}

fn stored(host: &Host) -> [u8; 32] {
    host.0
        .storage_get_for_test(&CONTRACT, b"c")
        .unwrap()
        .try_into()
        .unwrap()
}

#[test]
fn a_hidden_balance_is_debited_and_credited_by_proof() {
    let vm = Vm::new().unwrap();
    let code = vault();
    assert!(
        vm.validate(&code).is_err(),
        "maya_priv is not on the consensus surface"
    );
    let run = |host: Host, input: &[u8]| vm.execute_with_private(&code, CONTRACT, input, GAS, host);

    // The owner opens the vault with 1,000 hidden units.
    let (b0, b1, b2) = (blind(1), blind(2), blind(3));
    let c0 = commit(1_000, &b0).unwrap();
    let e = run(Host(MemoryState::at_height(1)), &input(0, 0, &c0, &[]));
    assert!(e.outcome.is_ok(), "{:?}", e.outcome);
    assert_eq!(stored(&e.state), to_bytes(&c0));

    // Debit 300, proved on the owner's device.
    let t = std::time::Instant::now();
    let (proof, c1) = prove_debit(1_000, &b0, 300, &b1).unwrap();
    let prove_time = t.elapsed();
    let e = run(e.state, &input(1, 300, &c1, proof.as_bytes()));
    let gas = e.outcome.as_ref().map(|o| o.gas_used);
    assert!(e.outcome.is_ok(), "{:?}", e.outcome);
    assert_eq!(stored(&e.state), to_bytes(&c1));
    let events: Vec<&Event> = e.state.0.events.iter().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(
        &events[0].data[1..],
        &300u64.to_le_bytes(),
        "only the amount is public"
    );

    // A replay of the same proof against the new state is refused.
    let replay = run(e.state, &input(1, 300, &c1, proof.as_bytes()));
    assert!(replay.outcome.is_err());
    // An overdraft cannot even be proved; a lie about the amount does not verify.
    assert!(prove_debit(700, &b1, 701, &b2).is_err());
    let (lie, c_lie) = prove_debit(700, &b1, 100, &b2).unwrap();
    let e = run(replay.state, &input(1, 50, &c_lie, lie.as_bytes()));
    assert!(matches!(e.outcome, Err(VmError::Trap(_))));
    assert_eq!(
        stored(&e.state),
        to_bytes(&c1),
        "state unchanged by a refused update"
    );

    // Credit 45: the hidden balance is now 745, and nobody but the owner knows.
    let (proof, c2) = prove_credit(700, &b1, 45, &b2).unwrap();
    let e = run(e.state, &input(2, 45, &c2, proof.as_bytes()));
    assert!(e.outcome.is_ok(), "{:?}", e.outcome);
    assert_eq!(
        from_bytes(&stored(&e.state)).unwrap(),
        commit(745, &b2).unwrap()
    );
    println!(
        "private debit: prove {prove_time:?} (desktop, debug build), proof {} bytes, contract call {gas:?} gas",
        proof.as_bytes().len()
    );
}
