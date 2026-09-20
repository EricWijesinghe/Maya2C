//! The zkML verification host function's contract with the rest of the VM:
//! its bounds, its price, and its answers.
//!
//! The verifier itself is not here. `maya-vm` links no pairing library — the
//! node supplies verification through [`crate::HostState::verify_zkml`], the
//! same seam that keeps chain types out of the VM. That keeps the VM's
//! dependency set to wasmtime, and it means a host that has not opted in has
//! no verifier at all rather than a disabled one.
//!
//! # The price is measured, and it is large on purpose
//!
//! Gas is wasmtime fuel, about one unit per wasm instruction, and fuel cannot
//! see native work. A host function that verifies a pairing-based proof does
//! ~6 ms of native work (release build, `crates/zkml-prover/benches/verify.rs`); a tight
//! guest loop on the same machine burns ~16.6 million fuel per millisecond
//! (`crates/vm/tests/fuel_calibration_tests.rs`). So the honest charge is on the order
//! of a hundred million fuel, and [`ZKML_VERIFY_BASE_FUEL`] is set at roughly
//! 9 ms' worth for headroom.
//!
//! That is fifteen times the VM's default gas ceiling, which means a contract
//! calling this must ask for a larger `gas_limit`. That is correct: a call that
//! verifies a proof *is* fifteen times as much work as the default call budget,
//! and a price that pretended otherwise would let a contract loop over
//! verifications at a fraction of what they cost the validators running it.
//!
//! The per-byte charge covers copying the buffers out of guest memory and
//! hashing the key. The verification itself does not grow with the byte
//! counts: the verifier accepts one circuit shape, reads a fixed number of
//! points, and refuses trailing bytes afterwards.
//!
//! # Charged before any work
//!
//! The fuel is taken first. A call that cannot pay traps without a byte having
//! been read and without a pairing having been computed — otherwise
//! "out of gas" would arrive after the work it was meant to bound.

use wasmtime::{Caller, Linker};

use crate::error::{Result, VmError};
use crate::host::HostState;
use crate::runtime::{CallContext, caller_memory, read_guest};

/// Length of a model identifier: a hash of the verifying key.
pub const ZKML_MODEL_ID_LEN: usize = 32;

/// Largest verifying key the host function will copy out of guest memory.
///
/// Must equal `maya_zkml::verify::MAX_VK_BYTES`; `tests/zkml_block_tests.rs`
/// pins the two together, since this crate cannot import that one.
pub const MAX_ZKML_VK_BYTES: usize = 16 * 1024;

/// Largest proof the host function will copy out of guest memory.
pub const MAX_ZKML_PROOF_BYTES: usize = 16 * 1024;

/// Most public inputs: the widest model input, plus the class.
pub const MAX_ZKML_PUBLIC_INPUTS: usize = 17;

/// Fuel charged per verification, before anything else happens.
pub const ZKML_VERIFY_BASE_FUEL: u64 = 150_000_000;

/// Fuel charged per byte of key and proof copied out of the guest.
pub const ZKML_VERIFY_FUEL_PER_BYTE: u64 = 16;

/// Fuel charged per public input.
pub const ZKML_VERIFY_FUEL_PER_PUBLIC_INPUT: u64 = 1_000;

/// What a verification costs, for the given buffer sizes.
///
/// Public so a contract author — and a test — can compute a sufficient gas
/// limit rather than guess one.
#[must_use]
pub const fn verification_fuel(vk_len: usize, proof_len: usize, public_inputs: usize) -> u64 {
    ZKML_VERIFY_BASE_FUEL
        + ZKML_VERIFY_FUEL_PER_BYTE * (vk_len as u64 + proof_len as u64)
        + ZKML_VERIFY_FUEL_PER_PUBLIC_INPUT * public_inputs as u64
}

/// The host's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZkmlVerdict {
    /// The proof establishes the claim. The guest sees `1`.
    Valid,
    /// The proof does not establish the claim — including a proof that does
    /// not parse, and a key that is not the model the contract named. The guest
    /// sees `0` and decides for itself.
    ///
    /// A bad proof is an answer, not a malfunction. Trapping on it would let
    /// anyone who can get a bad proof into a call abort the block carrying it.
    Invalid,
    /// The key could not be parsed or a buffer was out of bounds: the
    /// contract's own data is broken. The call traps.
    Malformed(String),
    /// No verifier here — below the activation height, or a host that never
    /// wired one in. The call traps with [`crate::VmError::ZkmlUnavailable`].
    Unavailable,
}

/// Bytes per public input in guest memory: a little-endian `i32`.
///
/// `i32` rather than `i64` because every public input is an int8 input or a
/// class index; a wider encoding would only be a wider space of values the
/// circuit must then refuse.
pub const ZKML_PUBLIC_INPUT_BYTES: usize = 4;

/// Records `error` and returns the trap that stops the guest.
///
/// Unlike most host functions here, which record a failure and return a
/// sentinel while the guest carries on to a failed call, this one traps at
/// once: the guest has just been charged for a verification it did not get,
/// and letting it run further on a `-1` it may not check is how that turns
/// into a contract that believes a proof was checked.
fn refuse<S: HostState>(
    caller: &mut Caller<'_, CallContext<S>>,
    error: VmError,
) -> wasmtime::Error {
    caller.data_mut().fail(error);
    wasmtime::Error::msg("host_verify_zkml_proof refused")
}

fn length(value: i32, what: &'static str, limit: usize) -> Result<usize> {
    let length = usize::try_from(value)
        .map_err(|_| VmError::InvalidHostCall(format!("negative {what} length {value}")))?;
    if length > limit {
        return Err(VmError::SizeLimit {
            what,
            actual: length,
            limit,
        });
    }
    Ok(length)
}

/// Registers `host_verify_zkml_proof`.
///
/// ```text
/// host_verify_zkml_proof(
///     model_ptr: i32,                 32-byte model id
///     vk_ptr: i32, vk_len: i32,       verifying key
///     public_ptr: i32, public_count: i32,   little-endian i32 each: inputs, then class
///     proof_ptr: i32, proof_len: i32,
/// ) -> i32                            1 valid, 0 invalid; traps otherwise
/// ```
///
/// The model id is a parameter, not something the host returns, for the same
/// reason `oracle_read` takes a staleness bound: a contract cannot verify a
/// proof without naming the model it expects, so "forgot to check which model
/// produced this" is unreachable rather than discouraged.
pub(crate) fn register<S: HostState + Send + 'static>(
    linker: &mut Linker<CallContext<S>>,
) -> Result<()> {
    linker
        .func_wrap(
            "env",
            "host_verify_zkml_proof",
            |mut caller: Caller<'_, CallContext<S>>,
             model_ptr: i32,
             vk_ptr: i32,
             vk_len: i32,
             public_ptr: i32,
             public_count: i32,
             proof_ptr: i32,
             proof_len: i32|
             -> wasmtime::Result<i32> {
                // Lengths first, because the price depends on them.
                let lengths = (|| {
                    Ok::<_, VmError>((
                        length(vk_len, "zkML verifying key", MAX_ZKML_VK_BYTES)?,
                        length(public_count, "zkML public inputs", MAX_ZKML_PUBLIC_INPUTS)?,
                        length(proof_len, "zkML proof", MAX_ZKML_PROOF_BYTES)?,
                    ))
                })();
                let (vk_len, public_count, proof_len) = match lengths {
                    Ok(lengths) => lengths,
                    Err(error) => return Err(refuse(&mut caller, error)),
                };

                // Then the charge, before a byte is read or a pairing computed.
                // Out of fuel is reported as wasmtime's own out-of-fuel trap, so
                // it classifies exactly like running out inside the guest.
                let cost = verification_fuel(vk_len, proof_len, public_count);
                let remaining = caller.get_fuel()?;
                if remaining < cost {
                    caller.set_fuel(0)?;
                    return Err(wasmtime::Trap::OutOfFuel.into());
                }
                caller.set_fuel(remaining - cost)?;

                let Some(memory) = caller_memory(&mut caller) else {
                    return Err(refuse(&mut caller, VmError::MissingExport("memory".into())));
                };
                let read =
                    |ptr: i32, len: usize| read_guest(&memory, &caller, ptr as u32, len as u32);
                let buffers = (|| {
                    Ok::<_, VmError>((
                        read(model_ptr, ZKML_MODEL_ID_LEN)?,
                        read(vk_ptr, vk_len)?,
                        read(public_ptr, public_count * ZKML_PUBLIC_INPUT_BYTES)?,
                        read(proof_ptr, proof_len)?,
                    ))
                })();
                let (model, vk, public_bytes, proof) = match buffers {
                    Ok(buffers) => buffers,
                    Err(error) => return Err(refuse(&mut caller, error)),
                };

                let mut model_id = [0u8; ZKML_MODEL_ID_LEN];
                model_id.copy_from_slice(&model);
                let public: Vec<i64> = public_bytes
                    .as_chunks::<ZKML_PUBLIC_INPUT_BYTES>()
                    .0
                    .iter()
                    .map(|c| i64::from(i32::from_le_bytes(*c)))
                    .collect();

                match caller
                    .data()
                    .state
                    .verify_zkml(&model_id, &vk, &public, &proof)
                {
                    ZkmlVerdict::Valid => Ok(1),
                    ZkmlVerdict::Invalid => Ok(0),
                    ZkmlVerdict::Malformed(why) => Err(refuse(
                        &mut caller,
                        VmError::InvalidHostCall(format!("zkML: {why}")),
                    )),
                    ZkmlVerdict::Unavailable => Err(refuse(&mut caller, VmError::ZkmlUnavailable)),
                }
            },
        )
        .map_err(|e| VmError::UnresolvedImport(e.to_string()))?;
    Ok(())
}
