//! Contract execution.
//!
//! ## ABI
//!
//! A contract exports:
//!
//! ```text
//! memory
//! input_ptr()    -> i32   address of its static input buffer
//! input_cap()    -> i32   capacity of that buffer
//! invoke(len:i32)-> i64   (out_ptr << 32) | out_len ; negative means failure
//! ```
//!
//! The host writes the call input into the contract's own buffer and passes the
//! length. There is no allocator in the ABI: a `#![no_std]` contract has no heap
//! by default, and requiring one would exclude the smallest contracts for no
//! benefit.
//!
//! ## Gas
//!
//! Gas maps one-to-one onto wasmtime fuel. Gas is a **halting bound**, not a
//! price — running out traps, the call fails, and every staged write is
//! discarded. Nothing is billed to the caller.
//!
//! ## Reverting
//!
//! Host writes go into the [`HostState`] the caller supplies. Because execution
//! either returns a result or an error, and the node applies state only on
//! success, a trap partway through leaves nothing behind. The
//! `gas_exhaustion_reverts_all_writes` test pins that.

use wasmtime::{Caller, Engine, Extern, Instance, Linker, Memory, Module, Store};

use crate::cache::ModuleCache;
use crate::config::{
    MAX_MEMORY_PAGES, MAX_MODULE_BYTES, PAGE_SIZE, config_digest, deterministic_engine,
};
use crate::error::{Result, VmError};
use crate::host::{
    Address, ContractId, Event, HostState, MAX_EVENT_BYTES, MAX_EVENTS, MAX_KEY_BYTES,
    MAX_VALUE_BYTES, RANDOMNESS_LEN, check_size,
};

/// A completed execution: the host state, plus what happened.
///
/// The state comes back either way. A caller that gets an error is expected to
/// discard it; one that succeeds commits it. Making that an explicit choice is
/// what keeps a trapped call from leaving partial writes behind.
pub struct Execution<S> {
    /// The host state after execution.
    pub state: S,
    /// What the call produced, or why it failed.
    pub outcome: Result<Outcome>,
}

/// Result of a completed contract call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Bytes the contract returned.
    pub output: Vec<u8>,
    /// Gas consumed.
    pub gas_used: u64,
    /// Events emitted, in order.
    pub events: Vec<Event>,
}

/// Per-call state threaded through host functions.
///
/// Owns the [`HostState`] rather than borrowing it: `Store<T>` requires
/// `T: 'static`, so a borrow cannot live here. [`Vm::execute`] takes the state
/// by value and returns it, which also makes it impossible to observe a
/// half-updated state after a trap — the caller decides whether to keep it.
pub(crate) struct CallContext<S: HostState> {
    pub(crate) state: S,
    contract: ContractId,
    events: Vec<Event>,
    /// First host-side failure. Host functions cannot return `Result` through
    /// the wasm boundary, so the error is recorded and the guest is trapped.
    failure: Option<VmError>,
    /// Ceiling enforced by the resource limiter.
    max_pages: usize,
}

impl<S: HostState> CallContext<S> {
    /// Records a host-side failure, keeping the first.
    pub(crate) fn fail(&mut self, error: VmError) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
    }
}

impl<S: HostState + Send + 'static> wasmtime::ResourceLimiter for CallContext<S> {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let limit = self.max_pages * PAGE_SIZE;
        if desired > limit {
            self.fail(VmError::MemoryLimit {
                requested: desired / PAGE_SIZE,
                limit: self.max_pages,
            });
            return Ok(false);
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        // Reference types are disabled, so tables should never grow.
        Ok(false)
    }
}

/// Reads a guest slice, refusing out-of-bounds offsets.
pub(crate) fn read_guest(
    memory: &Memory,
    store: &impl wasmtime::AsContext,
    ptr: u32,
    len: u32,
) -> Result<Vec<u8>> {
    let data = memory.data(store);
    let start = ptr as usize;
    let end = start
        .checked_add(len as usize)
        .ok_or(VmError::MemoryOutOfBounds {
            offset: ptr,
            length: len,
        })?;
    if end > data.len() {
        return Err(VmError::MemoryOutOfBounds {
            offset: ptr,
            length: len,
        });
    }
    Ok(data[start..end].to_vec())
}

/// Writes into guest memory, refusing out-of-bounds offsets.
fn write_guest(
    memory: &Memory,
    store: &mut impl wasmtime::AsContextMut,
    ptr: u32,
    bytes: &[u8],
) -> Result<()> {
    let data = memory.data_mut(store);
    let start = ptr as usize;
    let end = start
        .checked_add(bytes.len())
        .ok_or(VmError::MemoryOutOfBounds {
            offset: ptr,
            length: bytes.len() as u32,
        })?;
    if end > data.len() {
        return Err(VmError::MemoryOutOfBounds {
            offset: ptr,
            length: bytes.len() as u32,
        });
    }
    data[start..end].copy_from_slice(bytes);
    Ok(())
}

/// Fetches the guest's exported memory.
pub(crate) fn caller_memory<S: HostState + Send + 'static>(
    caller: &mut Caller<'_, CallContext<S>>,
) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(memory)) => Some(memory),
        _ => None,
    }
}

/// Registers imports beyond the consensus surface.
pub(crate) type Registrar<S> = fn(&mut Linker<CallContext<S>>) -> Result<()>;

/// A configured, reusable VM.
///
/// The [`Engine`] holds compiled-code caches and is comparatively expensive to
/// build, so a node constructs one and reuses it across calls.
pub struct Vm {
    engine: Engine,
    /// Compiled modules, so a contract is optimised once rather than per call.
    ///
    /// The engine's own caches do not cover this: `Module::new` runs Cranelift
    /// every time it is called, whatever the engine has seen before. See
    /// [`crate::cache`].
    cache: ModuleCache,
    /// Default gas ceiling when a caller does not specify one.
    default_gas: u64,
}

impl Vm {
    /// Builds a VM with the deterministic configuration.
    ///
    /// # Errors
    ///
    /// Returns [`VmError::EngineConfig`] if the engine cannot be built.
    pub fn new() -> Result<Self> {
        Ok(Self {
            engine: deterministic_engine()?,
            cache: ModuleCache::new(config_digest()),
            default_gas: 10_000_000,
        })
    }

    /// Builds a VM compiling to `tier`. Gas is identical in every tier —
    /// see [`crate::tier`] — so this is a node-local performance choice.
    ///
    /// # Errors
    ///
    /// Returns [`VmError::EngineConfig`] if the engine cannot be built.
    pub fn with_tier(tier: crate::tier::Tier) -> Result<Self> {
        let (engine, digest) = crate::tier::tier_engine(tier)?;
        Ok(Self {
            engine,
            cache: ModuleCache::new(digest),
            default_gas: 10_000_000,
        })
    }

    /// The underlying engine.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The module cache, for benchmarks and for an operator's telemetry.
    ///
    /// Nothing that decides anything reads it: a hit and a miss produce the same
    /// code, charge the same fuel and return the same result.
    #[must_use]
    pub fn cache(&self) -> &ModuleCache {
        &self.cache
    }

    /// Validates and compiles a module without running it.
    ///
    /// Deployment calls this so a malformed or disallowed module is rejected at
    /// deploy time, when exactly one transaction pays for the failure, rather
    /// than at every later call.
    ///
    /// # Errors
    ///
    /// Returns [`VmError::InvalidModule`] if the bytes are not valid wasm, use a
    /// disabled proposal, or exceed [`MAX_MODULE_BYTES`], and
    /// [`VmError::UnresolvedImport`] for an import the host does not provide.
    pub fn validate(&self, wasm: &[u8]) -> Result<()> {
        if wasm.len() > MAX_MODULE_BYTES {
            return Err(VmError::SizeLimit {
                what: "module",
                actual: wasm.len(),
                limit: MAX_MODULE_BYTES,
            });
        }
        let module =
            Module::new(&self.engine, wasm).map_err(|e| VmError::InvalidModule(e.to_string()))?;

        // Names only. The linker checks the *types* at instantiation, and it
        // has to: a signature mismatch is a different failure with a different
        // message. What is decided here is whether the name exists at all,
        // which is the question a deployer can answer and a caller cannot.
        for import in module.imports() {
            if import.module() != HOST_MODULE || !HOST_FUNCTIONS.contains(&import.name()) {
                return Err(VmError::UnresolvedImport(format!(
                    "{}::{}",
                    import.module(),
                    import.name()
                )));
            }
        }
        Ok(())
    }

    /// Default gas ceiling.
    #[must_use]
    pub fn default_gas(&self) -> u64 {
        self.default_gas
    }

    /// Executes `wasm`'s `invoke` entry point.
    ///
    /// Takes the host state by value and returns it inside [`Execution`],
    /// because `Store<T>` requires `T: 'static` and so cannot hold a borrow.
    /// The shape is useful in its own right: on failure the caller still gets
    /// the state back and decides explicitly whether to keep or discard the
    /// writes, rather than finding a shared state already half-modified.
    ///
    /// # Errors
    ///
    /// The returned [`Execution::outcome`] is [`VmError::OutOfGas`] when gas is
    /// exhausted, [`VmError::Trap`] for any other guest fault, or a load error
    /// if the module is invalid or missing required exports.
    #[must_use]
    pub fn execute<S: HostState + Send + 'static>(
        &self,
        wasm: &[u8],
        contract: ContractId,
        input: &[u8],
        gas_limit: u64,
        state: S,
    ) -> Execution<S> {
        match self.run(wasm, contract, input, gas_limit, state, |_| Ok(())) {
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

    /// Inner execution, returning the state alongside either result.
    ///
    /// `extra` registers imports beyond the consensus surface; [`Vm::execute`]
    /// passes none, `crate::resources` passes the `maya_res` module.
    #[allow(clippy::type_complexity)]
    pub(crate) fn run<S: HostState + Send + 'static>(
        &self,
        wasm: &[u8],
        contract: ContractId,
        input: &[u8],
        gas_limit: u64,
        state: S,
        extra: Registrar<S>,
    ) -> core::result::Result<(S, Outcome), (S, VmError)> {
        // Compile before building the store, so a bad module never reaches it.
        //
        // Through the cache: the same bytes under the same configuration
        // compile to the same code and charge the same fuel, so this is the one
        // place a contract's cost can fall without anything observable changing.
        // `a_cached_call_burns_identical_fuel` is what says so.
        let module = match self.cache.compile(&self.engine, wasm) {
            Ok(module) => module,
            Err(error) => return Err((state, error)),
        };

        let context = CallContext {
            state,
            contract,
            events: Vec::new(),
            failure: None,
            max_pages: MAX_MEMORY_PAGES,
        };

        let mut store = Store::new(&self.engine, context);
        store.limiter(|ctx| ctx);
        if let Err(e) = store.set_fuel(gas_limit) {
            return Err((
                store.into_data().state,
                VmError::EngineConfig(e.to_string()),
            ));
        }

        let mut linker: Linker<CallContext<S>> = Linker::new(&self.engine);
        if let Err(error) = register_host_functions(&mut linker).and_then(|()| extra(&mut linker)) {
            return Err((store.into_data().state, error));
        }

        let instance = match linker.instantiate(&mut store, &module) {
            Ok(instance) => instance,
            Err(e) => {
                let error = classify(&e, gas_limit);
                return Err((store.into_data().state, error));
            }
        };

        let invoked = self.invoke(&mut store, &instance, input, gas_limit);

        // A host function that failed traps the guest, but the useful error is
        // the one the host recorded, not the generic trap it produced.
        let recorded = store.data_mut().failure.take();

        let remaining = store.get_fuel().unwrap_or(0);
        let gas_used = gas_limit.saturating_sub(remaining);
        let events = std::mem::take(&mut store.data_mut().events);
        let data = store.into_data();

        match (recorded, invoked) {
            (Some(failure), _) => Err((data.state, failure)),
            (None, Err(error)) => Err((data.state, error)),
            (None, Ok(output)) => Ok((
                data.state,
                Outcome {
                    output,
                    gas_used,
                    events,
                },
            )),
        }
    }

    /// Writes the input into the guest's buffer and calls `invoke`.
    fn invoke<S: HostState + Send + 'static>(
        &self,
        store: &mut Store<CallContext<S>>,
        instance: &Instance,
        input: &[u8],
        gas_limit: u64,
    ) -> Result<Vec<u8>> {
        let memory = instance
            .get_memory(&mut *store, "memory")
            .ok_or_else(|| VmError::MissingExport("memory".to_string()))?;

        // Contracts that take no input need not export a buffer.
        if !input.is_empty() {
            let input_ptr = instance
                .get_typed_func::<(), i32>(&mut *store, "input_ptr")
                .map_err(|_| VmError::MissingExport("input_ptr".to_string()))?
                .call(&mut *store, ())
                .map_err(|e| classify(&e, gas_limit))?;
            let capacity = instance
                .get_typed_func::<(), i32>(&mut *store, "input_cap")
                .map_err(|_| VmError::MissingExport("input_cap".to_string()))?
                .call(&mut *store, ())
                .map_err(|e| classify(&e, gas_limit))?;

            if input.len() > capacity.max(0) as usize {
                return Err(VmError::InvalidHostCall(format!(
                    "input of {} bytes exceeds the contract's buffer of {capacity}",
                    input.len()
                )));
            }

            write_guest(&memory, &mut *store, input_ptr as u32, input)?;
        }

        let invoke = instance
            .get_typed_func::<i32, i64>(&mut *store, "invoke")
            .map_err(|_| VmError::MissingExport("invoke".to_string()))?;

        let packed = invoke
            .call(&mut *store, input.len() as i32)
            .map_err(|e| classify(&e, gas_limit))?;

        if packed < 0 {
            return Err(VmError::Trap(format!(
                "contract reported failure code {packed}"
            )));
        }

        // (ptr << 32) | len
        let out_ptr = ((packed as u64) >> 32) as u32;
        let out_len = (packed as u64 & 0xFFFF_FFFF) as u32;
        if out_len == 0 {
            return Ok(Vec::new());
        }

        read_guest(&memory, &*store, out_ptr, out_len)
    }
}

/// Maps a wasmtime error onto a VM error, separating gas exhaustion from faults.
fn classify(error: &wasmtime::Error, gas_limit: u64) -> VmError {
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>()
        && *trap == wasmtime::Trap::OutOfFuel
    {
        return VmError::OutOfGas { limit: gas_limit };
    }
    // The typed trap is the reliable signal; the string check is a fallback for
    // wrapped errors that lose the concrete type.
    let text = format!("{error:?}");
    if text.contains("all fuel consumed") || text.contains("OutOfFuel") {
        return VmError::OutOfGas { limit: gas_limit };
    }
    VmError::Trap(text)
}

/// The one module name a contract may import from.
pub const HOST_MODULE: &str = "env";

/// Every host function name `register_host_functions` registers, and so the
/// whole of what a contract may import.
///
/// Written out so [`Vm::validate`] can refuse an unresolvable import at
/// *deploy* rather than at first call. Without it a module importing
/// `env.call` — the import a re-entrancy attack needs — is stored on chain and
/// only fails when somebody invokes it, which turns a deployer's mistake into
/// every caller's. `crates/vm/tests/host_surface_tests.rs` checks the list against
/// what the linker actually registers, so the two cannot drift.
pub const HOST_FUNCTIONS: &[&str] = &[
    "block_height",
    // ADR-026: who signed the transaction. Live from genesis — no module
    // could import it before it existed, so no deployed contract changes.
    "caller",
    "get_balance",
    "storage_read",
    "storage_write",
    "block_randomness",
    "oracle_read",
    "oracle_feed_age",
    "emit_event",
    // Registered by `crate::zkml::register`, unconditionally and with no
    // feature gate, so it belongs here even though `ZKML_ACTIVATION_HEIGHT`
    // leaves it dark: a name the linker resolves and this list omits is a
    // contract that validates on no node at all.
    "host_verify_zkml_proof",
];

/// Registers every host function a contract may import.
///
/// The set is deliberately small and closed. Anything not listed here is
/// unreachable from a contract, which is what makes the sandbox a sandbox.
fn register_host_functions<S: HostState + Send + 'static>(
    linker: &mut Linker<CallContext<S>>,
) -> Result<()> {
    let wrap = |e: wasmtime::Error| VmError::UnresolvedImport(e.to_string());

    // block_height() -> i64
    linker
        .func_wrap(
            "env",
            "block_height",
            |caller: Caller<'_, CallContext<S>>| caller.data().state.block_height() as i64,
        )
        .map_err(wrap)?;

    // caller(out_ptr: i32) -> i32
    // Writes the signer's 32-byte address and returns 0, or returns -1 where
    // no transaction exists (a dry run). ADR-026.
    linker
        .func_wrap(
            "env",
            "caller",
            |mut caller: Caller<'_, CallContext<S>>, out_ptr: i32| -> i32 {
                let Some(address) = caller.data().state.caller() else {
                    return -1;
                };
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return -1;
                };
                match write_guest(&memory, &mut caller, out_ptr as u32, &address) {
                    Ok(()) => 0,
                    Err(error) => {
                        caller.data_mut().fail(error);
                        -1
                    }
                }
            },
        )
        .map_err(wrap)?;

    // get_balance(addr_ptr: i32) -> i64
    linker
        .func_wrap(
            "env",
            "get_balance",
            |mut caller: Caller<'_, CallContext<S>>, addr_ptr: i32| -> i64 {
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return 0;
                };
                match read_guest(&memory, &caller, addr_ptr as u32, 32) {
                    Ok(bytes) => {
                        let mut address: Address = [0u8; 32];
                        address.copy_from_slice(&bytes);
                        caller.data().state.balance_of(&address) as i64
                    }
                    Err(error) => {
                        caller.data_mut().fail(error);
                        0
                    }
                }
            },
        )
        .map_err(wrap)?;

    // storage_read(key_ptr, key_len, out_ptr, out_cap) -> i32
    // Returns the value length, or -1 when the key is absent. A caller that
    // supplies too small a buffer gets the required length back and can retry.
    linker
        .func_wrap(
            "env",
            "storage_read",
            |mut caller: Caller<'_, CallContext<S>>,
             key_ptr: i32,
             key_len: i32,
             out_ptr: i32,
             out_cap: i32|
             -> i32 {
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return -1;
                };
                let key = match read_guest(&memory, &caller, key_ptr as u32, key_len as u32) {
                    Ok(key) => key,
                    Err(error) => {
                        caller.data_mut().fail(error);
                        return -1;
                    }
                };
                if let Err(error) = check_size("storage key", key.len(), MAX_KEY_BYTES) {
                    caller.data_mut().fail(error);
                    return -1;
                }

                let contract = caller.data().contract;
                let Some(value) = caller.data().state.storage_get(&contract, &key) else {
                    return -1;
                };

                let length = value.len() as i32;
                if length > out_cap {
                    // Not an error: report the size so the guest can retry.
                    return length;
                }
                if let Err(error) = write_guest(&memory, &mut caller, out_ptr as u32, &value) {
                    caller.data_mut().fail(error);
                    return -1;
                }
                length
            },
        )
        .map_err(wrap)?;

    // storage_write(key_ptr, key_len, val_ptr, val_len)
    linker
        .func_wrap(
            "env",
            "storage_write",
            |mut caller: Caller<'_, CallContext<S>>,
             key_ptr: i32,
             key_len: i32,
             val_ptr: i32,
             val_len: i32| {
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return;
                };
                let key = match read_guest(&memory, &caller, key_ptr as u32, key_len as u32) {
                    Ok(key) => key,
                    Err(error) => return caller.data_mut().fail(error),
                };
                let value = match read_guest(&memory, &caller, val_ptr as u32, val_len as u32) {
                    Ok(value) => value,
                    Err(error) => return caller.data_mut().fail(error),
                };
                if let Err(error) = check_size("storage key", key.len(), MAX_KEY_BYTES) {
                    return caller.data_mut().fail(error);
                }
                if let Err(error) = check_size("storage value", value.len(), MAX_VALUE_BYTES) {
                    return caller.data_mut().fail(error);
                }

                let contract = caller.data().contract;
                caller.data_mut().state.storage_set(&contract, key, value);
            },
        )
        .map_err(wrap)?;

    // block_randomness(out_ptr) -> i32
    // Writes 32 bytes and returns 0, or returns -1 when the chain has no
    // beacon. Nothing is written on failure, so a contract that ignores the
    // return value reads whatever was already in its buffer rather than a value
    // that looks like randomness.
    linker
        .func_wrap(
            "env",
            "block_randomness",
            |mut caller: Caller<'_, CallContext<S>>, out_ptr: i32| -> i32 {
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return -1;
                };
                let Some(randomness) = caller.data().state.block_randomness() else {
                    return -1;
                };
                if let Err(error) = write_guest(&memory, &mut caller, out_ptr as u32, &randomness) {
                    caller.data_mut().fail(error);
                    return -1;
                }
                RANDOMNESS_LEN as i32
            },
        )
        .map_err(wrap)?;

    // oracle_read(feed_ptr, max_age_blocks, out_ptr) -> i32
    //
    // `max_age_blocks` is a *parameter*, not a field of the result, and that is
    // the whole design. A contract cannot read a price without stating how
    // stale a price it will accept, so the failure mode where an author forgets
    // to check the age is unreachable rather than merely discouraged.
    //
    // Returns 8 on success, -1 for an unknown feed, and -2 for a stale one. The
    // two failures are distinguished because they call for different handling:
    // an unknown feed is a bug in the contract, a stale one is a fact about the
    // world that may resolve on its own.
    linker
        .func_wrap(
            "env",
            "oracle_read",
            |mut caller: Caller<'_, CallContext<S>>,
             feed_ptr: i32,
             max_age_blocks: i64,
             out_ptr: i32|
             -> i32 {
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return -1;
                };
                let feed_id = match read_guest(&memory, &caller, feed_ptr as u32, 32) {
                    Ok(bytes) => {
                        let mut id = [0u8; 32];
                        id.copy_from_slice(&bytes);
                        id
                    }
                    Err(error) => {
                        caller.data_mut().fail(error);
                        return -1;
                    }
                };

                let Some(feed) = caller.data().state.oracle_feed(&feed_id) else {
                    return -1;
                };

                // Saturating, so a feed somehow ahead of the chain — a reorg in
                // flight — reads as fresh rather than as a wrapped, enormous
                // age that would pass any bound.
                let age = caller
                    .data()
                    .state
                    .block_height()
                    .saturating_sub(feed.updated_height);
                // A negative bound is not "no bound": it is a caller who got
                // their arithmetic wrong, and answering it would be answering a
                // question nobody asked.
                if max_age_blocks < 0 || age > max_age_blocks as u64 {
                    return -2;
                }

                if let Err(error) = write_guest(
                    &memory,
                    &mut caller,
                    out_ptr as u32,
                    &feed.value.to_le_bytes(),
                ) {
                    caller.data_mut().fail(error);
                    return -1;
                }
                8
            },
        )
        .map_err(wrap)?;

    // oracle_feed_age(feed_ptr) -> i64
    // Blocks since the feed last moved, or -1 if it does not exist. For a
    // contract that wants to log or branch on staleness rather than simply be
    // refused by `oracle_read`.
    linker
        .func_wrap(
            "env",
            "oracle_feed_age",
            |mut caller: Caller<'_, CallContext<S>>, feed_ptr: i32| -> i64 {
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return -1;
                };
                let feed_id = match read_guest(&memory, &caller, feed_ptr as u32, 32) {
                    Ok(bytes) => {
                        let mut id = [0u8; 32];
                        id.copy_from_slice(&bytes);
                        id
                    }
                    Err(error) => {
                        caller.data_mut().fail(error);
                        return -1;
                    }
                };

                match caller.data().state.oracle_feed(&feed_id) {
                    Some(feed) => caller
                        .data()
                        .state
                        .block_height()
                        .saturating_sub(feed.updated_height)
                        as i64,
                    None => -1,
                }
            },
        )
        .map_err(wrap)?;

    // emit_event(topic_ptr, topic_len, data_ptr, data_len)
    linker
        .func_wrap(
            "env",
            "emit_event",
            |mut caller: Caller<'_, CallContext<S>>,
             topic_ptr: i32,
             topic_len: i32,
             data_ptr: i32,
             data_len: i32| {
                let Some(memory) = caller_memory(&mut caller) else {
                    caller
                        .data_mut()
                        .fail(VmError::MissingExport("memory".into()));
                    return;
                };
                let topic = match read_guest(&memory, &caller, topic_ptr as u32, topic_len as u32) {
                    Ok(topic) => topic,
                    Err(error) => return caller.data_mut().fail(error),
                };
                let data = match read_guest(&memory, &caller, data_ptr as u32, data_len as u32) {
                    Ok(data) => data,
                    Err(error) => return caller.data_mut().fail(error),
                };
                if let Err(error) = check_size("event payload", data.len(), MAX_EVENT_BYTES) {
                    return caller.data_mut().fail(error);
                }
                let emitted = caller.data().events.len();
                if emitted >= MAX_EVENTS {
                    // Events are unmetered output every node must store, so the
                    // count is capped independently of gas.
                    return caller.data_mut().fail(VmError::SizeLimit {
                        what: "event count",
                        actual: emitted + 1,
                        limit: MAX_EVENTS,
                    });
                }

                let contract = caller.data().contract;
                let event = Event {
                    contract,
                    topic,
                    data,
                };
                caller.data_mut().events.push(event.clone());
                caller.data_mut().state.emit(event);
            },
        )
        .map_err(wrap)?;

    // host_verify_zkml_proof(...) -> i32. In its own module, with its price.
    crate::zkml::register(linker)?;

    Ok(())
}
