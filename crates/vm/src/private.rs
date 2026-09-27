//! Private contract state: the `maya_priv` import module (Master Prompt 27).
//!
//! A contract keeps a private field as a 32-byte commitment in its ordinary
//! storage. The owner updates it off chain and proves the update
//! (`maya_zk_stark::private_state`); the contract asks the host to check the
//! proof and, on success, stores the new commitment. The chain sees
//! commitments and public amounts, never values.
//!
//! ```text
//! maya_priv.verify_debit(old_ptr, new_ptr, amount: i64, proof_ptr, proof_len) -> i32
//! maya_priv.verify_credit(old_ptr, new_ptr, amount: i64, proof_ptr, proof_len) -> i32
//!     1 valid, 0 invalid; traps on malformed arguments
//! ```
//!
//! The VM stays free of the proof system: the host implements
//! [`PrivateVerifier`]. Fuel is charged before verification, by proof size.
//!
//! **Not consensus.** Like `maya_res` (`crate::resources`), `maya_priv` is not
//! in `HOST_FUNCTIONS`: `Vm::validate` refuses a module importing it, and only
//! [`Vm::execute_with_private`] resolves it. On chain it needs an activation
//! height and an audit of the circuit (Master Prompt 27 §4).

use wasmtime::{Caller, Linker};

use crate::error::{Result, VmError};
use crate::host::{ContractId, HostState};
use crate::runtime::{CallContext, Execution, Vm, caller_memory, read_guest};

/// The import module name.
pub const PRIVATE_MODULE: &str = "maya_priv";
/// Largest proof accepted.
pub const MAX_PRIVATE_PROOF_BYTES: usize = 4 * 1024 * 1024;
/// Fuel charged per verification, before any per-byte charge.
pub const PRIVATE_VERIFY_BASE_FUEL: u64 = 1_000_000;
/// Fuel per KiB of proof: verification reads and hashes all of it.
pub const PRIVATE_VERIFY_FUEL_PER_KIB: u64 = 2_000;
const COMMITMENT_BYTES: u32 = 32;

/// Which direction a transition goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The value falls by the amount.
    Debit,
    /// The value rises by the amount.
    Credit,
}

/// Host state that can check private-state proofs.
pub trait PrivateVerifier: HostState {
    /// Whether `proof` shows `new` hides `old`'s value moved by `amount`.
    fn verify_transition(
        &self,
        direction: Direction,
        old: &[u8; 32],
        new: &[u8; 32],
        amount: u64,
        proof: &[u8],
    ) -> bool;
}

fn refuse<S: HostState>(
    caller: &mut Caller<'_, CallContext<S>>,
    error: VmError,
) -> wasmtime::Error {
    caller.data_mut().fail(error);
    wasmtime::Error::msg("maya_priv refused")
}

fn fuel_for(proof_len: usize) -> u64 {
    let kib = u64::try_from(proof_len.div_ceil(1024)).unwrap_or(u64::MAX);
    PRIVATE_VERIFY_BASE_FUEL.saturating_add(kib.saturating_mul(PRIVATE_VERIFY_FUEL_PER_KIB))
}

// Pointers and lengths cross the boundary as i32 and are reinterpreted.
#[allow(clippy::cast_sign_loss)]
fn verify<S: PrivateVerifier + Send + 'static>(
    caller: &mut Caller<'_, CallContext<S>>,
    direction: Direction,
    (old_ptr, new_ptr, amount, proof_ptr, proof_len): (i32, i32, i64, i32, i32),
) -> wasmtime::Result<i32> {
    let (Ok(amount), Ok(len)) = (u64::try_from(amount), usize::try_from(proof_len)) else {
        return Err(refuse(
            caller,
            VmError::InvalidHostCall("negative amount or length".into()),
        ));
    };
    if len > MAX_PRIVATE_PROOF_BYTES {
        return Err(refuse(
            caller,
            VmError::SizeLimit {
                what: "private-state proof",
                actual: len,
                limit: MAX_PRIVATE_PROOF_BYTES,
            },
        ));
    }
    let cost = fuel_for(len);
    let remaining = caller.get_fuel()?;
    if remaining < cost {
        caller.set_fuel(0)?;
        return Err(wasmtime::Trap::OutOfFuel.into());
    }
    caller.set_fuel(remaining - cost)?;
    let Some(memory) = caller_memory(caller) else {
        return Err(refuse(caller, VmError::MissingExport("memory".into())));
    };
    let read = |ptr: i32, n: u32| read_guest(&memory, &*caller, ptr as u32, n);
    let loaded = (|| {
        Ok::<_, VmError>((
            read(old_ptr, COMMITMENT_BYTES)?,
            read(new_ptr, COMMITMENT_BYTES)?,
            read(
                proof_ptr,
                u32::try_from(len).map_err(|_| VmError::InvalidHostCall("proof length".into()))?,
            )?,
        ))
    })();
    let (old, new, proof) = match loaded {
        Ok(v) => v,
        Err(e) => return Err(refuse(caller, e)),
    };
    let as_array = |v: &[u8]| <[u8; 32]>::try_from(v).unwrap_or([0; 32]);
    let ok = caller.data().state.verify_transition(
        direction,
        &as_array(&old),
        &as_array(&new),
        amount,
        &proof,
    );
    Ok(i32::from(ok))
}

fn register<S: PrivateVerifier + Send + 'static>(
    linker: &mut Linker<CallContext<S>>,
) -> Result<()> {
    let wrap = |e: wasmtime::Error| VmError::UnresolvedImport(e.to_string());
    for (name, direction) in [
        ("verify_debit", Direction::Debit),
        ("verify_credit", Direction::Credit),
    ] {
        linker
            .func_wrap(
                PRIVATE_MODULE,
                name,
                move |mut caller: Caller<'_, CallContext<S>>,
                      old: i32,
                      new: i32,
                      amount: i64,
                      proof: i32,
                      len: i32| {
                    verify(&mut caller, direction, (old, new, amount, proof, len))
                },
            )
            .map_err(wrap)?;
    }
    Ok(())
}

impl Vm {
    /// Executes like [`Vm::execute`], with the `maya_priv` imports resolved
    /// against `state`'s verifier.
    #[must_use]
    pub fn execute_with_private<S: PrivateVerifier + Send + 'static>(
        &self,
        wasm: &[u8],
        contract: ContractId,
        input: &[u8],
        gas_limit: u64,
        state: S,
    ) -> Execution<S> {
        match self.run(wasm, contract, input, gas_limit, state, register::<S>) {
            Ok((state, outcome)) => Execution {
                state,
                outcome: Ok(outcome),
            },
            Err((state, error)) => Execution {
                state,
                outcome: Err(error),
            },
        }
    }
}
