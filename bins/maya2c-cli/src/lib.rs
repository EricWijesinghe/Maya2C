//! The `maya2c` developer CLI (Master Prompt 24): the pieces that are worth
//! testing without a terminal.

pub mod debug;
pub mod dev;
pub mod fork;

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
    let exec = vm.execute(wasm, contract, input, gas_limit, Traced::new(state));
    let (state, timeline) = exec.state.finish();
    let (gas, outcome) = match &exec.outcome {
        Ok(out) => (
            Some(out.gas_used),
            format!("ok, {} output bytes", out.output.len()),
        ),
        Err(error) => (None, format!("failed: {error}")),
    };
    Ok((debug::Session::new(timeline, gas, outcome), state))
}
