//! The `maya2c` developer CLI (Master Prompt 24): the pieces that are worth
//! testing without a terminal.

pub mod custody_report;
pub mod dap;
pub mod debug;
pub mod dev;
pub mod fork;
pub mod lines;
pub mod vault;

use maya_vm::host::{ContractId, MemoryState};
use maya_vm::runtime::Vm;
use maya_vm::trace::Traced;

/// Runs `wasm` once, traced, and opens a debugger session on the recording.
///
/// # Errors
///
/// The VM could not be created. A failing call is not an error here: its
/// steps up to the failure are exactly what a debugger is for.
pub fn record(
    wasm: &[u8],
    contract: ContractId,
    input: &[u8],
    gas_limit: u64,
    state: MemoryState,
) -> anyhow::Result<(debug::Session, MemoryState)> {
    let vm = Vm::new()?;
    let exec = vm.execute_traced(wasm, contract, input, gas_limit, Traced::new(state));
    let (state, timeline) = exec.state.finish();
    // A module without DWARF, or with DWARF this reader cannot parse, keeps
    // host-call granularity; neither is a reason to refuse to debug it.
    let table = lines::LineTable::read(wasm).unwrap_or_default();
    let lines = timeline
        .sites()
        .iter()
        .map(|site| site.offset.and_then(|s| table.at(s).cloned()))
        .collect();
    // Gas between host calls: the fuel each step's call found left, against
    // the step before (the limit, for the first).
    let mut left = gas_limit;
    let step_gas = timeline
        .sites()
        .iter()
        .map(|site| {
            site.fuel_left.map(|now| {
                let spent = left.saturating_sub(now);
                left = now;
                spent
            })
        })
        .collect();
    let (gas, outcome) = match &exec.outcome {
        Ok(out) => (
            Some(out.gas_used),
            format!("ok, {} output bytes", out.output.len()),
        ),
        Err(error) => (None, format!("failed: {error}")),
    };
    Ok((
        debug::Session::new(timeline, gas, outcome)
            .with_lines(lines)
            .with_step_gas(step_gas),
        state,
    ))
}

/// What a debug session runs: the call and the chain context it sees.
pub struct CallSpec {
    /// The contract's wasm.
    pub wasm: Vec<u8>,
    /// Call input.
    pub input: Vec<u8>,
    /// The signer the contract sees.
    pub caller: Option<[u8; 32]>,
    /// Block height the contract sees.
    pub height: u64,
    /// Gas limit.
    pub gas: u64,
}

/// The contract id every debug session runs under.
pub const DEBUG_CONTRACT: ContractId = [0xDB; 32];

/// Parses a hex argument, `0x` optional.
///
/// # Errors
///
/// Not hex.
pub fn parse_hex(what: &str, text: &str) -> anyhow::Result<Vec<u8>> {
    hex::decode(text.trim_start_matches("0x"))
        .map_err(|e| anyhow::anyhow!("{what} is not hex: {e}"))
}

/// Parses a 32-byte hex caller.
///
/// # Errors
///
/// Not hex, or not 32 bytes.
pub fn parse_caller(text: &str) -> anyhow::Result<[u8; 32]> {
    parse_hex("caller", text)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("caller must be 32 bytes"))
}

/// Records `spec` under [`DEBUG_CONTRACT`].
///
/// # Errors
///
/// As [`record`].
pub fn record_spec(spec: &CallSpec) -> anyhow::Result<debug::Session> {
    let mut state = MemoryState::at_height(spec.height);
    state.caller = spec.caller;
    Ok(record(&spec.wasm, DEBUG_CONTRACT, &spec.input, spec.gas, state)?.0)
}
