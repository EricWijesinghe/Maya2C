//! Share mining on a GPU: the full dataset resident on the device, nonces
//! mixed in batches by `hashimoto.wgsl`, each result closed on the CPU and
//! checked against the share target exactly as the node checks it.
//!
//! The shader's agreement with `hashimoto_light` is pinned by
//! `hal/wgpu-miner/tests/gpu_validation.rs`; a share this finds and the node
//! refuses would be a parity bug there, not here.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use custom_l1_node::crypto::dag::cache::Cache;
use custom_l1_node::crypto::dag::dataset::Dataset;
use custom_l1_node::crypto::pow::meets_target;
use custom_l1_node::state::mining::DagProfile;
use maya_wgpu_miner::gpu::{self, GpuMiner};
use maya_wgpu_miner::reference::{flatten, proof_from_mix, seed_words};

use crate::share::GpuOpts;

/// The device miner, rebuilt only when the DAG epoch changes: the dataset is
/// the expensive part (4 GiB on mainnet), shares change every mining epoch.
pub struct GpuEngine {
    opts: GpuOpts,
    current: Option<(DagProfile, u64, Arc<GpuMiner>)>,
}

/// What the worker shares with the loop that watches it.
pub struct Lane {
    pub header: [u8; 32],
    pub target: [u8; 32],
    pub base: u64,
    pub stop: Arc<AtomicBool>,
    pub hashes: Arc<AtomicU64>,
    pub found: mpsc::Sender<u64>,
    pub failure: Arc<Mutex<Option<String>>>,
}

impl GpuEngine {
    #[must_use]
    pub fn new(opts: GpuOpts) -> Self {
        Self {
            opts,
            current: None,
        }
    }

    /// The adapter's name, for the `start` event.
    ///
    /// # Errors
    ///
    /// No adapter at `opts.device`.
    pub fn adapter_name(&self) -> Result<String, String> {
        gpu::adapters()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|a| a.index == self.opts.device)
            .map(|a| a.name)
            .ok_or_else(|| format!("no GPU adapter at index {}", self.opts.device))
    }

    fn miner_for(
        &mut self,
        dag: DagProfile,
        dag_epoch: u64,
        cache: &Cache,
    ) -> Result<Arc<GpuMiner>, String> {
        if let Some((d, e, miner)) = &self.current
            && *d == dag
            && *e == dag_epoch
        {
            return Ok(Arc::clone(miner));
        }
        // Drop the old dataset first: two resident at once would need twice
        // the VRAM at every DAG epoch change.
        self.current = None;
        let dataset = Dataset::generate(cache).map_err(|e| e.to_string())?;
        let words = flatten(dataset.items());
        let miner = GpuMiner::new(
            self.opts.device,
            &words,
            dataset.pages(),
            self.opts.workgroup_size,
        )
        .map_err(|e| e.to_string())?;
        let miner = Arc::new(miner);
        self.current = Some((dag, dag_epoch, Arc::clone(&miner)));
        Ok(miner)
    }

    /// Starts one worker that feeds the device until `lane.stop` is set.
    ///
    /// # Errors
    ///
    /// The dataset or the device could not be built.
    pub fn spawn(
        &mut self,
        dag: DagProfile,
        dag_epoch: u64,
        cache: &Cache,
        lane: Lane,
    ) -> Result<JoinHandle<()>, String> {
        let miner = self.miner_for(dag, dag_epoch, cache)?;
        let batch = u64::from(self.opts.batch.max(1));
        Ok(thread::spawn(move || {
            if let Err(error) = feed(&miner, batch, &lane) {
                if let Ok(mut slot) = lane.failure.lock() {
                    *slot = Some(error);
                }
                lane.stop.store(true, Ordering::Relaxed);
            }
        }))
    }
}

/// Dispatches batches of consecutive nonces and reports the ones that are
/// shares.
fn feed(miner: &GpuMiner, batch: u64, lane: &Lane) -> Result<(), String> {
    let mut start = lane.base;
    while !lane.stop.load(Ordering::Relaxed) {
        let seeds: Vec<_> = (0..batch)
            .map(|i| seed_words(&lane.header, start.wrapping_add(i)))
            .collect();
        let mixes = miner
            .mixes(&seeds)
            .map_err(|e| format!("GPU dispatch: {e}"))?;
        for (offset, mix) in (0u64..).zip(&mixes) {
            let nonce = start.wrapping_add(offset);
            let proof = proof_from_mix(&lane.header, nonce, mix);
            if meets_target(&proof.result, &lane.target) && lane.found.send(nonce).is_err() {
                return Ok(());
            }
        }
        lane.hashes.fetch_add(batch, Ordering::Relaxed);
        start = start.wrapping_add(batch);
    }
    Ok(())
}
