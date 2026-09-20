//! The device side of the miner: a safe wrapper over four C functions.
//!
//! ## Why the FFI surface is this small
//!
//! Everything CUDA — allocation, upload, kernel launches, synchronisation —
//! lives in `kernels/dag.cu` and is compiled by nvcc. What crosses into Rust is
//! four functions taking plain pointers and lengths, so the `unsafe` in this
//! file is four calls with obvious preconditions rather than a hand-written
//! binding to the CUDA runtime API.
//!
//! ## What owns what
//!
//! [`GpuDag`] owns one opaque context, which owns the device's copy of the
//! cache, the 4 GiB dataset and the small search buffers. `Drop` frees them.
//! The context is not `Sync`: a CUDA context is bound to the thread that
//! created it, and sharing one across threads is how a miner ends up with two
//! launches racing on the same result buffer.

use crate::blake3_ref::key_words;
use crate::dag::{ITEM_WORDS, Item, Keys, Params};
use crate::error::{MinerError, Result};

/// Opaque handle to the device-side state. Defined in `kernels/dag.cu`.
#[repr(C)]
struct MayaDagContext {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn maya_dag_create(
        out: *mut *mut MayaDagContext,
        device: i32,
        cache_words: *const u32,
        cache_items: u32,
        dataset_pages: u32,
        item_key: *const u32,
        mix_key: *const u32,
    ) -> i32;

    fn maya_dag_search(
        ctx: *mut MayaDagContext,
        seed_words: *const u32,
        mix_key: *const u32,
        start_nonce: u64,
        nonce_count: u64,
        target: *const u8,
        found_nonce: *mut u64,
        found: *mut i32,
    ) -> i32;

    fn maya_dag_read_items(ctx: *mut MayaDagContext, first: u32, count: u32, out: *mut u32) -> i32;

    fn maya_dag_destroy(ctx: *mut MayaDagContext);

    fn maya_dag_device_count(count: *mut i32) -> i32;
}

/// Number of CUDA devices visible to this process.
///
/// Zero when the driver reports none, rather than an error: a machine with no
/// GPU is a normal thing for this binary to run on, and the caller's next move
/// is to mine on the CPU, not to fail.
#[must_use]
pub fn device_count() -> i32 {
    let mut count = 0i32;
    // SAFETY: `count` is a valid, aligned, initialised `i32` that outlives the
    // call, which is the function's only precondition. It writes at most one
    // `i32` through the pointer and returns.
    let status = unsafe { maya_dag_device_count(&raw mut count) };
    if status == 0 { count } else { 0 }
}

/// A materialised dataset on one CUDA device.
pub struct GpuDag {
    ctx: *mut MayaDagContext,
    params: Params,
    keys: Keys,
    epoch: u64,
}

impl GpuDag {
    /// Uploads `cache` and generates the epoch's dataset on `device`.
    ///
    /// The dataset is built on the device, never transferred: 4 GiB over PCIe
    /// takes longer than generating it does, and the host would have to hold a
    /// second copy to send.
    ///
    /// # Errors
    ///
    /// Returns [`MinerError::Cuda`] if the device cannot be selected, the
    /// allocations do not fit, or a kernel fails to launch. A 4 GiB dataset
    /// needs a card with appreciably more than 4 GiB of VRAM.
    ///
    /// # Panics
    ///
    /// Panics if `cache` is not exactly the size `params` calls for. This is
    /// the precondition the FFI call below relies on: the device is told to
    /// read `cache_items` items from the pointer, so a short slice would be an
    /// out-of-bounds read on the *host* side of a `cudaMemcpy`. A caller that
    /// built its cache with [`crate::dag::generate_cache`] at these parameters
    /// always satisfies it.
    pub fn create(device: i32, epoch: u64, params: Params, cache: &[u32]) -> Result<Self> {
        let keys = Keys::derive();
        let item_key = key_words(&keys.item);
        let mix_key = key_words(&keys.mix);
        let cache_items = params.cache_items();

        assert_eq!(
            cache.len(),
            cache_items as usize * ITEM_WORDS,
            "the cache must be exactly the size these parameters call for"
        );

        let mut ctx: *mut MayaDagContext = std::ptr::null_mut();
        // SAFETY: `ctx` is a valid out-parameter; `cache` is a live slice of at
        // least `cache_items * ITEM_WORDS` words, which the assertion above and
        // the caller's use of `generate_cache` establish; the two key pointers
        // are eight-word arrays live for the duration of the call. The callee
        // reads the cache and writes only through `ctx`.
        let status = unsafe {
            maya_dag_create(
                &raw mut ctx,
                device,
                cache.as_ptr(),
                cache_items,
                params.dataset_pages(),
                item_key.as_ptr(),
                mix_key.as_ptr(),
            )
        };

        if status != 0 || ctx.is_null() {
            return Err(MinerError::Cuda(describe(status)));
        }

        Ok(Self {
            ctx,
            params,
            keys,
            epoch,
        })
    }

    /// The epoch this dataset was built for.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The parameters it was built at.
    #[must_use]
    pub fn params(&self) -> Params {
        self.params
    }

    /// Searches `count` nonces from `start`, returning the winner if there is
    /// one.
    ///
    /// One launch, one synchronisation. `count` is the batch size and decides
    /// how long the GPU is busy before the host hears anything, so a caller
    /// that needs to react to a new chain tip promptly should keep batches to
    /// tens of milliseconds of work rather than seconds.
    ///
    /// # Errors
    ///
    /// Returns [`MinerError::Cuda`] if the launch or a transfer fails.
    pub fn search(
        &self,
        seed: &[u8; 32],
        start: u64,
        count: u64,
        target: &[u8; 32],
    ) -> Result<Option<u64>> {
        let seed_words = key_words(seed);
        let mix_key = key_words(&self.keys.mix);

        let mut nonce = 0u64;
        let mut found = 0i32;
        // SAFETY: `self.ctx` is non-null and was produced by `maya_dag_create`,
        // which is the callee's precondition; the four array pointers are live
        // for the call; `nonce` and `found` are initialised out-parameters. The
        // callee writes only through those two.
        let status = unsafe {
            maya_dag_search(
                self.ctx,
                seed_words.as_ptr(),
                mix_key.as_ptr(),
                start,
                count,
                target.as_ptr(),
                &raw mut nonce,
                &raw mut found,
            )
        };

        if status != 0 {
            return Err(MinerError::Cuda(describe(status)));
        }
        Ok((found != 0).then_some(nonce))
    }

    /// Copies `count` dataset items back from the device, starting at `first`.
    ///
    /// Exists for the parity test that compares a GPU-generated dataset against
    /// the CPU reference. The miner never needs this — the dataset exists to be
    /// read by the kernel — and copying all of it would take longer than
    /// generating it twice.
    ///
    /// # Errors
    ///
    /// Returns [`MinerError::Cuda`] if the transfer fails.
    pub fn read_items(&self, first: u32, count: u32) -> Result<Vec<Item>> {
        let mut words = vec![0u32; count as usize * ITEM_WORDS];
        // SAFETY: `words` is a live, initialised allocation of exactly
        // `count * ITEM_WORDS` words, which is the length the callee copies.
        let status = unsafe { maya_dag_read_items(self.ctx, first, count, words.as_mut_ptr()) };

        if status != 0 {
            return Err(MinerError::Cuda(describe(status)));
        }

        let (items, _) = words.as_chunks::<ITEM_WORDS>();
        Ok(items.to_vec())
    }
}

impl Drop for GpuDag {
    fn drop(&mut self) {
        // SAFETY: `self.ctx` came from `maya_dag_create` and has not been freed
        // — nothing else in this type frees it, and `GpuDag` is not `Copy`, so
        // this runs exactly once per context.
        unsafe { maya_dag_destroy(self.ctx) };
    }
}

/// Turns a shim status code into something an operator can act on.
fn describe(status: i32) -> String {
    match status {
        -1 => "device allocation failed: a 4 GiB dataset needs a card with more \
               than 4 GiB of free VRAM"
            .to_string(),
        -2 => "a host/device transfer failed".to_string(),
        -3 => "a kernel launch failed".to_string(),
        -4 => "invalid arguments passed to the CUDA shim".to_string(),
        other => format!("the CUDA shim returned status {other}"),
    }
}
