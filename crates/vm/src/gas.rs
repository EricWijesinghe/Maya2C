//! What a contract call pays for work the fuel meter cannot see (ADR-142).
//!
//! Fuel counts wasm operators, roughly one unit each (bulk memory operators
//! one unit per byte, wasmtime 48's default). It does not see native work:
//! compiling the module, instantiating it, or the body of a host function. A
//! host function that does more native work than its one `call` operator
//! costs is a loop that buys time below its price, and a block of such loops
//! is a block that takes longer to validate than its gas says.
//!
//! So every one of those is charged here, in fuel, before the work is done.
//!
//! ## How the numbers were chosen
//!
//! One rule: **about one nanosecond of measured worst-case native work per
//! unit of fuel**, on the machine `crates/vm/tests/gas_price_calibration_tests.rs`
//! and `crates/node/tests/vm_gas_tests.rs` ran on (dev profile, which leaves
//! this crate and the node unoptimised, so the figures are pessimistic). Plain
//! guest code measured 0.3–0.7 ns per fuel and bulk memory 0.01 ns per fuel,
//! so guest work is already inside the rule; the constants below bring native
//! work inside it too. The per-block cap then turns "1 ns per fuel" into a
//! time bound: 250 M gas is about 250 ms of the slowest priced work, a
//! quarter of the 1 s block time. The measured figures and the derivation are
//! in ADR-142.
//!
//! One exception to the rule: [`STORAGE_WRITE_BYTE_FUEL`] is priced by what a
//! written byte costs every node *forever* (it is stored and folded into the
//! state root), not by the nanoseconds of the write. At 240 fuel a byte, a
//! full block of gas writes at most ~1 MiB of new contract state.
//!
//! ## Refunds
//!
//! There are none, so there is nothing to bound. Unused gas is never billed in
//! the first place (gas is a halting and per-block bound, not a fee), and an
//! overwrite or a zero-length write is charged like any other write. A future
//! fee per gas that refunds cleared storage must cap the refund at a fraction
//! of the gas used, or a refund becomes a way to buy block time at a discount.
//!
//! ## Determinism
//!
//! Every charge is a function of lengths the call states or the value the host
//! returns, never of time, cache state or the execution tier. The same call
//! costs the same gas under Cranelift and Pulley
//! (`crates/vm/tests/tier_differential_tests.rs`).

use wasmtime::Caller;

use crate::host::HostState;
use crate::runtime::CallContext;

/// Charged once per call, before the module is instantiated: building the
/// store and the linker, instantiating, and the entry and exit calls.
///
/// Measured: 47–50 µs for an empty contract with a warm module cache.
pub const CALL_BASE_FUEL: u64 = 50_000;

/// Charged per byte of the module on a **cold** call: the first call to a
/// contract in a block, which may compile it.
///
/// Measured: 222–432 ns per byte for three module shapes compiled by
/// Cranelift 48 (432 was the worst run). Priced at the worst, rounded up.
pub const CODE_BYTE_FUEL: u64 = 450;

/// Charged per byte of the module on a **warm** call: a later call to a
/// contract already compiled in the same block. Instantiation still copies
/// the module's data segments, which bulk memory measures at well under a
/// nanosecond a byte.
pub const WARM_CODE_BYTE_FUEL: u64 = 1;

/// Charged on every host function call, before its body runs.
///
/// Measured: 93–562 ns a call for the host functions with no byte-sized
/// argument (`get_balance_u256` was the worst), against one unit of fuel for
/// the `call` operator before this existed.
pub const HOST_CALL_FUEL: u64 = 600;

/// Charged on every `storage_read`, before the key is looked up.
///
/// A lookup in the node is a RocksDB point read (the storage is no longer
/// loaded whole before the call). Measured with the node's host in
/// `crates/node/tests/vm_gas_timing_tests.rs`: a block of ~49,000 hits took
/// 78 ms, ~1.6 µs a read with everything else in the block. 5,000 leaves
/// room for a read that misses RocksDB's block cache.
pub const STORAGE_READ_BASE_FUEL: u64 = 5_000;

/// Charged per byte of the key and of the value a `storage_read` returns.
pub const STORAGE_READ_BYTE_FUEL: u64 = 1;

/// Charged on every `storage_write`, before the key and value are read.
pub const STORAGE_WRITE_BASE_FUEL: u64 = 5_000;

/// Charged per byte of the key and the value a `storage_write` stores.
///
/// Priced by persistence, not time: `MAX_BLOCK_GAS / 240` is ~1 MiB of new
/// contract state per block at most.
pub const STORAGE_WRITE_BYTE_FUEL: u64 = 240;

/// Charged per byte of an event's topic and payload, before they are copied.
/// The topic had no size bound, so before this a call could copy its whole
/// 16 MiB memory into a topic 64 times for 64 units of fuel.
pub const EVENT_BYTE_FUEL: u64 = 1;

/// What a call pays before its first instruction runs.
#[must_use]
pub const fn call_fuel(code_len: usize, warm: bool) -> u64 {
    let per_byte = if warm {
        WARM_CODE_BYTE_FUEL
    } else {
        CODE_BYTE_FUEL
    };
    CALL_BASE_FUEL.saturating_add(per_byte.saturating_mul(code_len as u64))
}

/// What a deployment counts against its block's gas: validating a module
/// compiles it, which is the cold-call compile without the call.
#[must_use]
pub const fn deploy_fuel(code_len: usize) -> u64 {
    call_fuel(code_len, false)
}

/// `base + per_byte × bytes`, saturating.
#[must_use]
pub const fn priced(base: u64, per_byte: u64, bytes: u64) -> u64 {
    base.saturating_add(per_byte.saturating_mul(bytes))
}

/// Takes `cost` fuel from the running call, or traps it out of fuel.
///
/// Out of fuel is wasmtime's own trap, so a host charge that cannot be paid
/// classifies exactly like running out inside the guest.
///
/// # Errors
///
/// The out-of-fuel trap when less than `cost` remains, or a fuel accessor
/// failure.
pub(crate) fn charge<S: HostState>(
    caller: &mut Caller<'_, CallContext<S>>,
    cost: u64,
) -> wasmtime::Result<()> {
    let remaining = caller.get_fuel()?;
    if remaining < cost {
        caller.set_fuel(0)?;
        return Err(wasmtime::Trap::OutOfFuel.into());
    }
    caller.set_fuel(remaining - cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cold_call_pays_for_every_byte_of_its_module() {
        assert_eq!(call_fuel(0, false), CALL_BASE_FUEL);
        assert_eq!(call_fuel(1_000, false), CALL_BASE_FUEL + 450_000);
        assert_eq!(call_fuel(1_000, true), CALL_BASE_FUEL + 1_000);
        assert_eq!(deploy_fuel(1_000), call_fuel(1_000, false));
    }

    #[test]
    fn a_price_saturates_rather_than_wrapping() {
        // A wrapped price would be a near-free charge for an enormous length.
        assert_eq!(priced(1, u64::MAX, 2), u64::MAX);
        assert_eq!(call_fuel(usize::MAX, false), u64::MAX);
    }
}
