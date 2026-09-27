//! Gas conversion: every engine's cost in Maya VM fuel.
//!
//! Fuel is the Maya VM's unit (wasmtime fuel, one per WebAssembly
//! instruction). The ratios below are fixed integers — consensus cannot
//! depend on a measurement taken at runtime — and they were **chosen from a
//! measurement**: `tests/multivm_tests.rs::the_conversion_ratios_are_within_2x_of_measured_cost`
//! times each engine per unit on the same machine and checks each ratio stays
//! within a factor of two of the measured cost, so a ratio that drifts from
//! reality fails a test instead of mispricing a contract.

/// Maya VM gas charged per unit of EVM gas. Measured 19.2–22.9 over three
/// warm runs (2026-09-27, Windows 11, Intel family 6 model 198, release):
/// ~1.1 ns per EVM gas against ~0.05 ns per Maya VM gas unit. (A first,
/// cold measurement said 1.6: it timed wasmtime compiling the module, not
/// running it.)
pub const FUEL_PER_EVM_GAS: u64 = 20;

/// Maya VM gas charged per SBF compute unit (one per instruction). Measured
/// 47.9–61.6 on the same runs: the SBF *interpreter* costs 2–3.4 ns per
/// instruction beside compiled WebAssembly.
pub const FUEL_PER_SBF_CU: u64 = 50;

/// EVM gas in fuel, saturating.
#[must_use]
pub const fn evm_gas_to_fuel(gas: u64) -> u64 {
    gas.saturating_mul(FUEL_PER_EVM_GAS)
}

/// SBF compute units in fuel, saturating.
#[must_use]
pub const fn sbf_cu_to_fuel(cu: u64) -> u64 {
    cu.saturating_mul(FUEL_PER_SBF_CU)
}
