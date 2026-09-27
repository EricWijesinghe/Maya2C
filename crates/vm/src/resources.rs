//! Resource-safe execution: the `maya_res` import module (Master Prompt 23).
//!
//! [`Vm::execute_with_resources`] runs a contract with five extra imports
//! that move value only through [`maya_contract_safety::Runtime`]:
//!
//! ```text
//! maya_res.self() -> i32                                 this contract's id
//! maya_res.balance(holder: i32, kind: i32) -> i64
//! maya_res.transfer(from: i32, to: i32, kind: i32, amount: i64)
//! maya_res.mint(kind: i32, to: i32, amount: i64)
//! maya_res.burn(kind: i32, from: i32, amount: i64)
//! ```
//!
//! The contract acts as itself: moving someone else's units needs their Move
//! capability, minting and burning need the kind's capability. A broken rule
//! traps the guest with [`VmError::Resource`], and the whole invocation runs
//! inside [`Runtime::call`], so a trap, running out of gas, or a declared
//! invariant failing at the end reverts every resource change the call made.
//!
//! **Not consensus.** `maya_res` is not in [`crate::runtime::HOST_FUNCTIONS`]:
//! [`Vm::validate`] refuses a module importing it, and [`Vm::execute`] does
//! not resolve it. Putting it on chain needs resource balances under the state
//! root, a state prefix (invariant 25) and an activation height — none exist.
//! Re-entrancy cannot arise here at all: the VM has no cross-contract call
//! import (`host_surface_tests`), so [`Runtime::call`]'s re-entry refusal is
//! exercised by `crates/contract-safety` alone.

use maya_contract_safety::{Fault, Id, Runtime};
use wasmtime::{Caller, Linker};

use crate::error::{Result, VmError};
use crate::host::{ContractId, HostState};
use crate::runtime::{CallContext, Execution, Outcome, Vm};

/// The import module name.
pub const RESOURCE_MODULE: &str = "maya_res";

/// Fuel charged per value-moving import, on top of the guest's own. A flat
/// price: each touches at most two balances and one capability.
pub const RESOURCE_OP_FUEL: u64 = 1_000;

/// Host state that also carries the resource runtime.
pub trait ResourceState: HostState {
    /// The resource runtime.
    fn resources(&mut self) -> &mut Runtime;
    /// The executing contract's id in that runtime.
    fn actor(&self) -> Id;
}

fn refuse<S: HostState>(
    caller: &mut Caller<'_, CallContext<S>>,
    error: VmError,
) -> wasmtime::Error {
    caller.data_mut().fail(error);
    wasmtime::Error::msg("maya_res refused")
}

fn charge<S: HostState>(caller: &mut Caller<'_, CallContext<S>>) -> wasmtime::Result<()> {
    let remaining = caller.get_fuel()?;
    if remaining < RESOURCE_OP_FUEL {
        caller.set_fuel(0)?;
        return Err(wasmtime::Trap::OutOfFuel.into());
    }
    caller.set_fuel(remaining - RESOURCE_OP_FUEL)
}

/// Charges, checks the amount, and applies `op`; a fault traps the guest.
fn apply<S: ResourceState>(
    caller: &mut Caller<'_, CallContext<S>>,
    amount: i64,
    op: impl FnOnce(&mut Runtime, Id, u128) -> core::result::Result<(), Fault>,
) -> wasmtime::Result<()> {
    charge(caller)?;
    let Ok(amount) = u128::try_from(amount) else {
        return Err(refuse(
            caller,
            VmError::InvalidHostCall(format!("negative amount {amount}")),
        ));
    };
    let actor = caller.data().state.actor();
    match op(caller.data_mut().state.resources(), actor, amount) {
        Ok(()) => Ok(()),
        Err(fault) => Err(refuse(caller, VmError::Resource(fault))),
    }
}

// Ids and kinds cross the boundary as i32 and are reinterpreted as u32.
#[allow(clippy::cast_sign_loss)]
fn id(value: i32) -> u32 {
    value as u32
}

fn register<S: ResourceState + Send + 'static>(linker: &mut Linker<CallContext<S>>) -> Result<()> {
    let wrap = |e: wasmtime::Error| VmError::UnresolvedImport(e.to_string());
    linker
        .func_wrap(
            RESOURCE_MODULE,
            "self",
            |caller: Caller<'_, CallContext<S>>| {
                #[allow(clippy::cast_possible_wrap)]
                let actor = caller.data().state.actor() as i32;
                actor
            },
        )
        .map_err(wrap)?;
    linker
        .func_wrap(
            RESOURCE_MODULE,
            "balance",
            |mut caller: Caller<'_, CallContext<S>>, holder: i32, kind: i32| {
                let held = caller
                    .data_mut()
                    .state
                    .resources()
                    .state
                    .balance(id(holder), id(kind));
                i64::try_from(held).unwrap_or(i64::MAX)
            },
        )
        .map_err(wrap)?;
    linker
        .func_wrap(
            RESOURCE_MODULE,
            "transfer",
            |mut caller: Caller<'_, CallContext<S>>, from: i32, to: i32, kind: i32, amount: i64| {
                apply(&mut caller, amount, |rt, actor, n| {
                    rt.transfer(actor, id(from), id(to), id(kind), n)
                })
            },
        )
        .map_err(wrap)?;
    linker
        .func_wrap(
            RESOURCE_MODULE,
            "mint",
            |mut caller: Caller<'_, CallContext<S>>, kind: i32, to: i32, amount: i64| {
                apply(&mut caller, amount, |rt, actor, n| {
                    rt.mint(actor, id(kind), id(to), n)
                })
            },
        )
        .map_err(wrap)?;
    linker
        .func_wrap(
            RESOURCE_MODULE,
            "burn",
            |mut caller: Caller<'_, CallContext<S>>, kind: i32, from: i32, amount: i64| {
                apply(&mut caller, amount, |rt, actor, n| {
                    rt.burn(actor, id(kind), id(from), n)
                })
            },
        )
        .map_err(wrap)?;
    Ok(())
}

impl Vm {
    /// Executes like [`Vm::execute`], with the `maya_res` imports resolved
    /// against `state`'s resource runtime. The invocation is one
    /// [`Runtime::call`] on the actor: any failure — a broken rule, a trap,
    /// out of gas, or a declared invariant at the end — restores the
    /// runtime's balances, supply and capabilities to what they were before.
    #[must_use]
    pub fn execute_with_resources<S: ResourceState + Send + 'static>(
        &self,
        wasm: &[u8],
        contract: ContractId,
        input: &[u8],
        gas_limit: u64,
        mut state: S,
    ) -> Execution<S> {
        let mut runtime = std::mem::take(state.resources());
        let actor = state.actor();
        let mut slot = Some(state);
        let mut outcome: Option<Result<Outcome>> = None;
        let verdict = runtime.call(actor, "invoke", |rt| {
            let Some(mut state) = slot.take() else {
                return Err(Fault::Aborted);
            };
            // The runtime, with this call already on its stack, goes into the
            // store for the guest's imports, and comes back out afterwards.
            std::mem::swap(rt, state.resources());
            let (mut state, result) =
                match self.run(wasm, contract, input, gas_limit, state, register::<S>) {
                    Ok((state, out)) => (state, Ok(out)),
                    Err((state, error)) => (state, Err(error)),
                };
            std::mem::swap(rt, state.resources());
            slot = Some(state);
            let verdict = match &result {
                Ok(_) => Ok(()),
                Err(VmError::Resource(fault)) => Err(fault.clone()),
                Err(_) => Err(Fault::Aborted),
            };
            outcome = Some(result);
            verdict
        });
        let mut state = slot.expect("the call body always returns the state");
        *state.resources() = runtime;
        let outcome = match (verdict, outcome) {
            (_, Some(Err(error))) => Err(error),
            (Err(fault), _) => Err(VmError::Resource(fault)),
            (Ok(()), Some(Ok(out))) => Ok(out),
            (Ok(()), None) => Err(VmError::Resource(Fault::Aborted)),
        };
        Execution { state, outcome }
    }
}
