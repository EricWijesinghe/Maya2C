//! FINDING (2026-11-23, `vm_tier_differential` fuzz run on Linux,
//! reproduced on Windows): how deep a contract may recurse depends on the
//! execution tier, not on the module.
//!
//! The guest stack is bounded in *bytes* (`MAX_STACK_BYTES`, 512 KiB, through
//! wasmtime's `max_wasm_stack`), and a frame's size is whatever the compiler
//! makes it. Pulley's frames are larger than Cranelift x86-64's, so on the
//! module below Pulley hits "call stack exhausted" with less work done:
//! at gas 211,281 Cranelift runs out of gas and Pulley exhausts its stack.
//!
//! What it means for consensus: the node runs Cranelift only (ADR-016), so
//! the Pulley gap affects no chain today. The same mechanism applies across
//! CPU architectures, because Cranelift's frame sizes on aarch64 are not
//! x86-64's: a contract that recurses to a depth between the two limits and
//! then returns would succeed on one validator and trap on the other. No
//! aarch64 run exists yet, so that is NOT VERIFIED, not ruled out. The fix
//! is a call-depth limit counted in frames rather than bytes, which is a
//! consensus design decision (reports/05-vm.md); this test stays ignored
//! until one is made, and `cargo test -- --ignored` still shows the gap.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_vm::host::{MemoryState, OracleValue};
use maya_vm::zkml::ZKML_MODEL_ID_LEN;
use maya_vm::{Address, Amount, ContractId, Event, HostState, Tier, Vm, ZkmlVerdict};

/// The host state the fuzzer drew for the diverging input: one oracle feed
/// and a zkML verifier answering `Invalid`.
struct FoundHost(MemoryState);

impl HostState for FoundHost {
    fn balance_of(&self, address: &Address) -> Amount {
        self.0.balance_of(address)
    }
    fn caller(&self) -> Option<Address> {
        self.0.caller()
    }
    fn block_height(&self) -> u64 {
        self.0.block_height()
    }
    fn block_randomness(&self) -> Option<[u8; 32]> {
        self.0.block_randomness()
    }
    fn oracle_feed(&self, feed_id: &[u8; 32]) -> Option<OracleValue> {
        self.0.oracle_feed(feed_id)
    }
    fn verify_zkml(
        &self,
        _: &[u8; ZKML_MODEL_ID_LEN],
        _: &[u8],
        _: &[i64],
        _: &[u8],
    ) -> ZkmlVerdict {
        ZkmlVerdict::Invalid
    }
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.0.storage_get(contract, key)
    }
    fn storage_set(&mut self, contract: &ContractId, key: Vec<u8>, value: Vec<u8>) {
        self.0.storage_set(contract, key, value);
    }
    fn emit(&mut self, event: Event) {
        self.0.emit(event);
    }
}

fn found_host() -> FoundHost {
    let mut state = MemoryState::at_height(7);
    state.caller = Some([3u8; 32]);
    state.randomness = Some([5u8; 32]);
    state.set_balance([3u8; 32], 1_000_000);
    state.feeds.insert(
        [0u8; 32],
        OracleValue {
            value: Amount::from_u64(17_868_022_691_004_938_231),
            updated_height: 7,
            round: 16_059_276_563_815_331_831,
            observations: 222,
        },
    );
    FoundHost(state)
}

const INPUT: [u8; 21] = [
    216, 216, 216, 216, 242, 255, 255, 255, 255, 255, 247, 98, 108, 111, 106, 107, 95, 114, 97,
    110, 100,
];

fn outcome(tier: Tier, code: &[u8], gas: u64) -> String {
    let vm = Vm::with_tier(tier).expect("engine");
    match vm
        .execute(code, [9u8; 32], &INPUT, gas, found_host())
        .outcome
    {
        Ok(o) => format!("ok, gas {}", o.gas_used),
        Err(maya_vm::VmError::Trap(text)) => text
            .find("wasm trap:")
            .map_or(text.as_str(), |at| text[at..].lines().next().unwrap_or(""))
            .to_string(),
        Err(other) => format!("{other:?}"),
    }
}

#[test]
#[ignore = "known divergence: stack depth depends on the tier's frame sizes; see the FINDING note"]
fn deep_recursion_ends_the_same_way_in_every_tier() {
    let code = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "tests/tier_findings/2866d568ee415a83e6229795090299c24d1325b664a5fc188ba0683e3651c965.wasm",
    ))
    .expect("finding module");
    for gas in [211_281u64, 300_000, 10_000_000] {
        assert_eq!(
            outcome(Tier::Pulley, &code, gas),
            outcome(Tier::Cranelift, &code, gas),
            "gas {gas}: pulley vs cranelift"
        );
    }
}
