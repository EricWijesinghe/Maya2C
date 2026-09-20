//! Compiling a contract once instead of on every call.
//!
//! ## What this is, and what it is not
//!
//! It is **not** a JIT. The VM has been a JIT since it was written: `Config` at
//! `crate::config` sets `Strategy::Cranelift`, so wasmtime compiles every module
//! to native x86-64 or aarch64 before it runs, and there is no interpreter in
//! this tree to be slower than.
//!
//! What was missing is that the compile happened **again on every call**.
//! `Vm::call` handed the same bytes to `Module::new` each time and Cranelift
//! optimised them from scratch. This holds the result.
//!
//! ## Why a cache cannot change gas
//!
//! Gas is wasmtime fuel (invariant 21), and fuel is instrumentation the compiler
//! injects per Wasm operator. The same bytes under the same configuration
//! produce the same instrumentation, so a cached module charges exactly what a
//! freshly compiled one charges. `a_cached_call_burns_identical_fuel` asserts
//! that rather than assuming it, because "gas is unchanged" is the entire safety
//! argument for caching anything on a consensus path.
//!
//! ## The configuration is part of the key
//!
//! Not just the bytecode. A cache keyed on bytes alone would, after a change to
//! `deterministic_config` — NaN canonicalisation, a proposal flag, the stack cap
//! — keep serving modules compiled under the *old* settings until the process
//! restarted. Two nodes would then be running the same contract under two
//! compilers, which is the one way a cache can fork a chain.
//!
//! So the key is `BLAKE3(config digest || bytecode)`, and a configuration change
//! invalidates every entry by construction rather than by anybody remembering.
//!
//! ## Bounded, and evicting the least recently used
//!
//! An unbounded cache is a node that dies from the number of distinct contracts
//! it has seen — which is a number an attacker chooses, by deploying junk. So
//! there is a cap, and the entry not touched for longest goes first: a contract
//! called once by an attacker should not evict one called every block.
//!
//! ## Not shared between engines
//!
//! A `Module` is bound to the `Engine` that compiled it, and instantiating it
//! against another is an error rather than a subtle wrong answer. The cache
//! lives inside `Vm` beside its engine for that reason, and the config digest in
//! the key is a second guard on the same thing.

use std::collections::HashMap;
use std::sync::Mutex;

use wasmtime::{Engine, Module};

use crate::error::{Result, VmError};

/// The most compiled modules one VM holds.
///
/// 256 modules of a few hundred kilobytes each is tens of megabytes — small
/// next to what a node already holds, and far more than the working set of
/// contracts a block touches. Chosen as a bound rather than a tuning: the
/// number that matters is that one exists.
pub const MAX_CACHED_MODULES: usize = 256;

/// A cache key: the configuration and the bytecode, hashed together.
type Key = [u8; 32];

/// One cached module and when it was last used.
struct Entry {
    module: Module,
    /// Monotonic counter, not a clock. A clock would make eviction depend on
    /// wall time, and this runs inside block execution where nothing may.
    touched: u64,
}

/// Compiled modules, keyed by configuration and bytecode.
pub struct ModuleCache {
    /// A digest of the engine configuration, mixed into every key.
    config_digest: [u8; 32],
    entries: Mutex<HashMap<Key, Entry>>,
    /// Advances on every lookup, so "least recently used" has a meaning that
    /// does not involve reading a clock.
    clock: Mutex<u64>,
    hits: Mutex<u64>,
    misses: Mutex<u64>,
}

impl ModuleCache {
    /// A cache for an engine, tagged with a digest of its configuration.
    ///
    /// The caller supplies the digest because only the caller knows what the
    /// configuration was — wasmtime exposes no stable fingerprint of a `Config`.
    /// `crate::config::config_digest` is what produces it.
    #[must_use]
    pub fn new(config_digest: [u8; 32]) -> Self {
        Self {
            config_digest,
            entries: Mutex::new(HashMap::new()),
            clock: Mutex::new(0),
            hits: Mutex::new(0),
            misses: Mutex::new(0),
        }
    }

    /// How many compiles this cache has saved, and how many it has not.
    ///
    /// For benchmarks and for an operator wondering whether the cap is right.
    /// Not consensus state and not read by anything that decides anything.
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        (
            *self.hits.lock().unwrap_or_else(|e| e.into_inner()),
            *self.misses.lock().unwrap_or_else(|e| e.into_inner()),
        )
    }

    /// How many modules are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The compiled module for these bytes, compiling if it is not held.
    ///
    /// # Errors
    ///
    /// Returns [`VmError::InvalidModule`] for bytes wasmtime will not compile —
    /// the same error `Module::new` would have produced, because a cache must
    /// not turn a compile failure into something else.
    ///
    /// A poisoned lock is recovered from rather than propagated: the cache holds
    /// no invariant a panicking thread could have broken half-way, since every
    /// entry is inserted whole.
    pub fn compile(&self, engine: &Engine, wasm: &[u8]) -> Result<Module> {
        let key = self.key(wasm);

        {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = entries.get_mut(&key) {
                entry.touched = self.tick();
                *self.hits.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                return Ok(entry.module.clone());
            }
        }

        // Compiled outside the lock. Cranelift on a large module takes long
        // enough that holding the map would serialise every other contract call
        // in the process behind it.
        let module =
            Module::new(engine, wasm).map_err(|error| VmError::InvalidModule(error.to_string()))?;
        *self.misses.lock().unwrap_or_else(|e| e.into_inner()) += 1;

        let touched = self.tick();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Another thread may have compiled the same module while this one was
        // working. Both results are the same code, so either is correct and the
        // duplicate work is simply lost.
        if entries.len() >= MAX_CACHED_MODULES
            && !entries.contains_key(&key)
            && let Some(coldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| *key)
        {
            entries.remove(&coldest);
        }
        entries.insert(
            key,
            Entry {
                module: module.clone(),
                touched,
            },
        );
        Ok(module)
    }

    /// Drops every entry.
    ///
    /// For a caller that has changed something the digest does not cover, and
    /// for tests that need a cold cache without building a second VM.
    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    /// The key these bytes have under this configuration.
    fn key(&self, wasm: &[u8]) -> Key {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"maya-vm-module-cache-v1:");
        hasher.update(&self.config_digest);
        // Length-prefixed, so no bytecode can be extended to collide with
        // another by absorbing the boundary.
        hasher.update(&(wasm.len() as u64).to_le_bytes());
        hasher.update(wasm);
        *hasher.finalize().as_bytes()
    }

    /// The next value of the monotonic counter.
    fn tick(&self) -> u64 {
        let mut clock = self.clock.lock().unwrap_or_else(|error| error.into_inner());
        *clock += 1;
        *clock
    }
}

impl std::fmt::Debug for ModuleCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (hits, misses) = self.stats();
        formatter
            .debug_struct("ModuleCache")
            .field("held", &self.len())
            .field("hits", &hits)
            .field("misses", &misses)
            .finish()
    }
}
