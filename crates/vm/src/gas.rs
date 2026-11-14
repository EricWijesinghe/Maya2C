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
//! One rule: **about one nanosecond of measured native work per unit of
//! fuel, for the slowest shape of each kind of work that was measured**, on
//! the machine `crates/vm/tests/gas_price_calibration_tests.rs`,
//! `crates/vm/tests/gas_adversarial_calibration_tests.rs` and
//! `crates/node/tests/vm_gas_timing_tests.rs` ran on. Plain guest code
//! measured 0.2–0.7 ns per fuel and bulk memory 0.01 ns per fuel, so guest
//! work is already inside the rule; the constants below bring native work
//! inside it too. The per-block cap then turns "1 ns per fuel" into a time
//! bound. The measured figures, the shapes that were *not* measured, and the
//! derivation are in ADR-142.
//!
//! Compiling is priced by the module's shape as well as its length
//! ([`crate::shape`]): per function, per declared local, per control
//! operator and per call. Priced by length alone, the review of 2026-11-14
//! built modules that compiled more than a thousand times slower than their
//! price (many locals, many empty functions, long runs of branches).
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
use crate::shape::ModuleShape;

/// Charged once per call, before the module is instantiated: building the
/// store and the linker, instantiating, and the entry and exit calls.
///
/// Measured: 47–50 µs for an empty contract with a warm module cache.
pub const CALL_BASE_FUEL: u64 = 50_000;

/// Charged per byte of the module on a **cold** call: the first call to a
/// contract in a block, which may compile it.
///
/// Measured: 222–432 ns per byte for three straight-line module shapes
/// compiled by Cranelift 48 (432 was the worst run). Priced at the worst,
/// rounded up. What a byte does not cover, its shape does: the four prices
/// below.
pub const CODE_BYTE_FUEL: u64 = 450;

/// Charged per function, imported or defined, on a cold call.
///
/// Measured (release build, 2026-11-14): 53 µs per empty function, 120 µs
/// per empty *exported* function (the exported one also compiles a
/// trampoline). Priced at the exported case, since which functions escape is
/// not counted.
pub const FUNCTION_FUEL: u64 = 125_000;

/// Charged per declared local, on a cold call.
///
/// Measured: 68–162 ns per local (2,000 functions of 1,024 locals was the
/// worst, function overhead included).
pub const LOCAL_FUEL: u64 = 170;

/// Charged per control operator (`block`, `loop`, `if`, `else`, `br`,
/// `br_if`, `br_table`, `return`), on a cold call.
///
/// Measured: 18–25 µs per `if` or `loop` at up to 1,000 in one function, in
/// two or three bytes each; past a few thousand in one function the time per
/// operator grows, which is what [`crate::shape::MAX_CONTROL_PER_FUNCTION`]
/// stops.
pub const CONTROL_FUEL: u64 = 30_000;

/// Charged per `call` or `call_indirect`, on a cold call.
///
/// Measured: 5.1–6.1 µs a call in two bytes.
pub const CALL_OP_FUEL: u64 = 8_000;

/// Charged per value a function's own type, a typed `block`/`if` or a
/// call's callee type carries ([`crate::shape::ModuleShape::arity`]), on a
/// cold call. Function types carry at most
/// [`crate::shape::MAX_TYPE_ARITY`] values.
///
/// Measured, release: typed blocks, `if`s and calls at the bounds compile in
/// at most 0.96 ns per unit of their whole price (best of three runs; up to
/// 1.18 on the loaded shared machine). The control and call prices carry
/// most of it (ADR-142, "Multi-value").
pub const ARITY_FUEL: u64 = 2_500;

/// Charged per value a `loop` carries
/// ([`crate::shape::ModuleShape::loop_arity`]), on a cold call.
///
/// Measured: a loop's params cost compile time per value that grows with the
/// loops in the function, about 11.75 ns per value per loop, so 23.5 µs per
/// value at the 2,000 a function may hold (2,000 loops of 16 params: 823 ms,
/// release). Priced above that (ADR-142, "Multi-value").
pub const LOOP_ARITY_FUEL: u64 = 30_000;

/// Charged once the call returns, per 64 KiB page of linear memory it held
/// at most (its initial pages included). A call may hold no more pages than
/// what is left of its gas after the upfront price could pay for.
///
/// Measured: touching every 4 KiB of a fresh page costs 3.2–5.7 µs a page
/// (release, four runs), 5.2–7.1 µs (dev), in page faults and zeroing the
/// fuel meter does not see: 11–15 ns per unit of fuel before this price
/// existed. Priced at the worst release run, rounded up.
pub const MEMORY_PAGE_FUEL: u64 = 6_000;

/// The most a cold call may pay up front: what the largest module would pay
/// if its length were all it cost. A deployment whose module would cost more
/// is refused, so every deployed contract stays callable by a plaintext call
/// under the lowest cap governance may set (`vm.max_tx_gas`'s floor is this
/// plus 4,000,000). A revealed sealed call's cap is the sealed budget, a
/// quarter of the block cap: a module priced above that is never called cold
/// through an envelope (GAS-5).
pub const MAX_COLD_CALL_FUEL: u64 = call_fuel(crate::config::MAX_MODULE_BYTES, false);

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
/// 78 ms, ~1.6 µs a read with everything else in the block. That is the
/// warm read; one that may go to disk pays [`STORAGE_COLD_READ_FUEL`] too.
pub const STORAGE_READ_BASE_FUEL: u64 = 5_000;

/// Charged on a `storage_read` of a key the block has not read or written
/// before (`HostState::touch_storage`), on top of the base and before the
/// lookup: the read that can miss every cache and go to disk.
///
/// Measured with the node's RocksDB options, a 1 MiB block cache and direct
/// I/O (no OS cache) on this PC's SSD, 2026-11-14: 73–113 µs a read for
/// 1-byte values, present or absent (the 113 µs run was under load from
/// other builds, 13% over this price). Base plus this is 100,000 fuel a cold
/// read. A read whose SST index block does not fit the cache rereads the
/// index too (1.4–2.0 ms measured with 4 KiB values and a 1 MiB cache); the
/// node's 512 MiB cache holds the index blocks, and ADR-142 records the case
/// as unpriced.
pub const STORAGE_COLD_READ_FUEL: u64 = 95_000;

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
/// The topic is bounded too now (`MAX_EVENT_TOPIC_BYTES`); before ADR-142 a
/// call could copy its whole 16 MiB memory into a topic 64 times for 64
/// units of fuel.
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

/// The length part of a deployment's price: [`call_fuel`] cold. What a
/// deployment actually counts is [`cold_call_fuel`], which adds its shape.
#[must_use]
pub const fn deploy_fuel(code_len: usize) -> u64 {
    call_fuel(code_len, false)
}

/// What compiling a module of this shape costs beyond its length.
#[must_use]
pub const fn shape_fuel(shape: &ModuleShape) -> u64 {
    FUNCTION_FUEL
        .saturating_mul(shape.functions)
        .saturating_add(LOCAL_FUEL.saturating_mul(shape.locals))
        .saturating_add(CONTROL_FUEL.saturating_mul(shape.controls))
        .saturating_add(CALL_OP_FUEL.saturating_mul(shape.calls))
        .saturating_add(ARITY_FUEL.saturating_mul(shape.arity))
        .saturating_add(LOOP_ARITY_FUEL.saturating_mul(shape.loop_arity))
}

/// What a cold call of `wasm` pays before its first instruction, and what a
/// deployment of it counts: its length and its shape (GAS-3).
///
/// # Errors
///
/// What [`crate::shape::scan`] refuses: a module past a shape bound, or bytes
/// that are not a module.
pub fn cold_call_fuel(wasm: &[u8]) -> crate::Result<u64> {
    let shape = crate::shape::scan(wasm)?;
    Ok(call_fuel(wasm.len(), false).saturating_add(shape_fuel(&shape)))
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
    fn a_cold_call_pays_for_the_shape_of_its_module_too() {
        let wasm =
            wat::parse_str(r#"(module (func (local i64 i64) (block (br 0)) (call 0)) (func))"#)
                .unwrap();
        let shape = ModuleShape {
            functions: 2,
            locals: 2,
            controls: 2,
            calls: 1,
            arity: 0,
            loop_arity: 0,
        };
        assert_eq!(crate::shape::scan(&wasm).unwrap(), shape);
        assert_eq!(
            cold_call_fuel(&wasm).unwrap(),
            call_fuel(wasm.len(), false)
                + 2 * FUNCTION_FUEL
                + 2 * LOCAL_FUEL
                + 2 * CONTROL_FUEL
                + CALL_OP_FUEL
        );
        assert_eq!(MAX_COLD_CALL_FUEL, 235_979_600);
    }

    #[test]
    fn a_price_saturates_rather_than_wrapping() {
        // A wrapped price would be a near-free charge for an enormous length.
        assert_eq!(priced(1, u64::MAX, 2), u64::MAX);
        assert_eq!(call_fuel(usize::MAX, false), u64::MAX);
    }
}
