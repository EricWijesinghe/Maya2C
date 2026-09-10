//! `host_verify_zkml_proof`, from the guest's side.
//!
//! The verifier here is a fake that returns whatever verdict the test chose
//! and counts how often it was asked. That is deliberate: these tests are about
//! the host function's contract — what it charges, when it charges, what it
//! refuses before asking the verifier, and how each verdict reaches the guest.
//! Real proofs against the real verifier are in `tests/zkml_block_tests.rs`,
//! in the node's suite.

use std::cell::Cell;

use maya_vm::zkml::{
    MAX_ZKML_PROOF_BYTES, MAX_ZKML_VK_BYTES, ZKML_MODEL_ID_LEN, verification_fuel,
};
use maya_vm::{Address, ContractId, Event, HostState, MemoryState, Vm, VmError, ZkmlVerdict};

/// A host whose verifier answers `verdict` and remembers what it was shown.
struct FakeVerifier {
    inner: MemoryState,
    verdict: ZkmlVerdict,
    calls: Cell<u32>,
    seen_model: Cell<[u8; ZKML_MODEL_ID_LEN]>,
    seen_public: Cell<[i64; 5]>,
}

impl FakeVerifier {
    fn answering(verdict: ZkmlVerdict) -> Self {
        Self {
            inner: MemoryState::at_height(1),
            verdict,
            calls: Cell::new(0),
            seen_model: Cell::new([0; ZKML_MODEL_ID_LEN]),
            seen_public: Cell::new([0; 5]),
        }
    }
}

impl HostState for FakeVerifier {
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
    fn verify_zkml(
        &self,
        model_id: &[u8; ZKML_MODEL_ID_LEN],
        _vk: &[u8],
        public: &[i64],
        _proof: &[u8],
    ) -> ZkmlVerdict {
        self.calls.set(self.calls.get() + 1);
        self.seen_model.set(*model_id);
        let mut first = [0i64; 5];
        for (slot, value) in first.iter_mut().zip(public) {
            *slot = *value;
        }
        self.seen_public.set(first);
        self.verdict.clone()
    }
}

// Guest memory layout used by every contract below.
const MODEL_AT: u32 = 0;
const VK_AT: u32 = 64;
const PUBLIC_AT: u32 = 128;
const PROOF_AT: u32 = 256;
const RESULT_AT: u32 = 1024;

const VK_LEN: i32 = 8;
const PUBLIC_COUNT: i32 = 5;
const PROOF_LEN: i32 = 16;

/// A contract that makes one verification call with the given arguments and
/// returns its result as four little-endian bytes.
fn contract(vk_len: i32, public_count: i32, proof_len: i32, proof_ptr: u32) -> Vec<u8> {
    let call = format!(
        "(call $verify (i32.const {MODEL_AT}) (i32.const {VK_AT}) (i32.const {vk_len})          (i32.const {PUBLIC_AT}) (i32.const {public_count})          (i32.const {proof_ptr}) (i32.const {proof_len}))"
    );
    module_around(&call)
}

/// The same module with the verification call replaced by a constant — the
/// baseline that isolates what the call itself costs.
fn contract_without_call() -> Vec<u8> {
    module_around("(i32.const 1)")
}

fn module_around(result: &str) -> Vec<u8> {
    let wat = format!(
        r#"(module
            (import "env" "host_verify_zkml_proof"
                (func $verify (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            ;; model id: bytes 0x01, 0x02, ... 0x20
            (data (i32.const {MODEL_AT}) "\01\02\03\04\05\06\07\08\09\0a\0b\0c\0d\0e\0f\10\11\12\13\14\15\16\17\18\19\1a\1b\1c\1d\1e\1f\20")
            ;; public inputs: -128, 127, 0, 5, then class 2, as little-endian i32
            (data (i32.const {PUBLIC_AT}) "\80\ff\ff\ff\7f\00\00\00\00\00\00\00\05\00\00\00\02\00\00\00")
            (func (export "invoke") (param i32) (result i64)
                (i32.store (i32.const {RESULT_AT}) {result})
                (i64.or
                    (i64.shl (i64.const {RESULT_AT}) (i64.const 32))
                    (i64.const 4))))"#
    );
    wat::parse_str(&wat).expect("wat")
}

fn standard() -> Vec<u8> {
    contract(VK_LEN, PUBLIC_COUNT, PROOF_LEN, PROOF_AT)
}

const GENEROUS: u64 = 1_000_000_000;

fn run(
    wasm: &[u8],
    gas: u64,
    host: FakeVerifier,
) -> (Result<(Vec<u8>, u64), VmError>, FakeVerifier) {
    let vm = Vm::new().expect("vm");
    let execution = vm.execute(wasm, [7; 32], &[], gas, host);
    (
        execution.outcome.map(|o| (o.output, o.gas_used)),
        execution.state,
    )
}

#[test]
fn a_valid_proof_reaches_the_guest_as_one() {
    let (outcome, host) = run(
        &standard(),
        GENEROUS,
        FakeVerifier::answering(ZkmlVerdict::Valid),
    );
    let (output, _) = outcome.expect("call succeeds");
    assert_eq!(output, 1i32.to_le_bytes());
    assert_eq!(host.calls.get(), 1);
}

#[test]
fn an_invalid_proof_reaches_the_guest_as_zero_and_the_call_succeeds() {
    // Not a trap. A bad proof is an answer the contract acts on; trapping would
    // let anyone who can get a bad proof into a call abort the block.
    let (outcome, _) = run(
        &standard(),
        GENEROUS,
        FakeVerifier::answering(ZkmlVerdict::Invalid),
    );
    let (output, _) = outcome.expect("call succeeds");
    assert_eq!(output, 0i32.to_le_bytes());
}

#[test]
fn a_host_without_a_verifier_traps_rather_than_answering_zero() {
    // MemoryState has no verifier: the trait's default. "Unavailable" must not
    // look like "invalid", or a contract written for a live verifier would treat
    // every proof as bad on a node where the feature is off.
    let vm = Vm::new().expect("vm");
    let execution = vm.execute(
        &standard(),
        [7; 32],
        &[],
        GENEROUS,
        MemoryState::at_height(1),
    );
    assert_eq!(execution.outcome.err(), Some(VmError::ZkmlUnavailable));
}

#[test]
fn a_malformed_key_traps() {
    let verdict = ZkmlVerdict::Malformed("does not parse".into());
    let (outcome, _) = run(&standard(), GENEROUS, FakeVerifier::answering(verdict));
    assert!(matches!(outcome, Err(VmError::InvalidHostCall(m)) if m.contains("does not parse")));
}

#[test]
fn the_model_id_and_public_inputs_arrive_intact_and_signed() {
    let (outcome, host) = run(
        &standard(),
        GENEROUS,
        FakeVerifier::answering(ZkmlVerdict::Valid),
    );
    outcome.expect("call succeeds");
    let expected_model: [u8; 32] = core::array::from_fn(|i| i as u8 + 1);
    assert_eq!(host.seen_model.get(), expected_model);
    // -128 must arrive as -128, not 4294967168: the encoding is i32, sign
    // extended, and a zero-extending decoder would hand the circuit an input it
    // then refuses as out of int8 range.
    assert_eq!(host.seen_public.get(), [-128, 127, 0, 5, 2]);
}

#[test]
fn verification_is_charged_its_full_price() {
    // Measured as a difference against the same module without the call,
    // because the rest of the module is not free: wasmtime charges ~4,096 fuel
    // to initialise data segments, which has nothing to do with this function.
    let price = verification_fuel(VK_LEN as usize, PROOF_LEN as usize, PUBLIC_COUNT as usize);
    let (outcome, _) = run(
        &standard(),
        GENEROUS,
        FakeVerifier::answering(ZkmlVerdict::Valid),
    );
    let (_, with_call) = outcome.expect("call succeeds");
    let (outcome, _) = run(
        &contract_without_call(),
        GENEROUS,
        FakeVerifier::answering(ZkmlVerdict::Valid),
    );
    let (_, without_call) = outcome.expect("baseline succeeds");

    let charged = with_call - without_call;
    // The price, plus the handful of instructions that push seven arguments.
    assert!(
        (price..price + 16).contains(&charged),
        "the call cost {charged}; the price is {price}"
    );
}

#[test]
fn a_call_that_cannot_pay_traps_before_the_verifier_runs() {
    // The property the charge-first ordering exists for: out of gas must arrive
    // *before* the work it bounds, not after it.
    let price = verification_fuel(VK_LEN as usize, PROOF_LEN as usize, PUBLIC_COUNT as usize);
    let (outcome, host) = run(
        &standard(),
        price - 1,
        FakeVerifier::answering(ZkmlVerdict::Valid),
    );
    assert_eq!(outcome.err(), Some(VmError::OutOfGas { limit: price - 1 }));
    assert_eq!(
        host.calls.get(),
        0,
        "the verifier ran for a call that could not pay"
    );
}

#[test]
fn oversized_buffers_are_refused_before_the_verifier_runs() {
    for (vk, proof) in [
        (MAX_ZKML_VK_BYTES as i32 + 1, PROOF_LEN),
        (VK_LEN, MAX_ZKML_PROOF_BYTES as i32 + 1),
    ] {
        let wasm = contract(vk, PUBLIC_COUNT, proof, PROOF_AT);
        let (outcome, host) = run(&wasm, GENEROUS, FakeVerifier::answering(ZkmlVerdict::Valid));
        assert!(
            matches!(outcome, Err(VmError::SizeLimit { .. })),
            "{outcome:?}"
        );
        assert_eq!(host.calls.get(), 0);
    }
    let wasm = contract(VK_LEN, 18, PROOF_LEN, PROOF_AT);
    let (outcome, _) = run(&wasm, GENEROUS, FakeVerifier::answering(ZkmlVerdict::Valid));
    assert!(matches!(outcome, Err(VmError::SizeLimit { .. })));
}

#[test]
fn a_negative_length_is_refused() {
    let wasm = contract(-1, PUBLIC_COUNT, PROOF_LEN, PROOF_AT);
    let (outcome, host) = run(&wasm, GENEROUS, FakeVerifier::answering(ZkmlVerdict::Valid));
    assert!(matches!(outcome, Err(VmError::InvalidHostCall(_))));
    assert_eq!(host.calls.get(), 0);
}

#[test]
fn a_buffer_past_the_end_of_guest_memory_is_refused() {
    // One page is 65536 bytes; a proof starting 8 bytes before the end cannot
    // hold 16.
    let wasm = contract(VK_LEN, PUBLIC_COUNT, PROOF_LEN, 65_536 - 8);
    let (outcome, host) = run(&wasm, GENEROUS, FakeVerifier::answering(ZkmlVerdict::Valid));
    assert!(matches!(outcome, Err(VmError::MemoryOutOfBounds { .. })));
    assert_eq!(host.calls.get(), 0);
}
