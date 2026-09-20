//! GPU compute accuracy against the node's own hashimoto.
//!
//! # What "accuracy" means here
//!
//! Not "close". Identical bytes. Proof of work is consensus: a digest that
//! differs by one bit from `crypto::dag::hashimoto_light` produces blocks the
//! network rejects — silently, and only once a solution is finally found, which
//! is the worst possible moment to learn about it.
//!
//! So every assertion below is an equality against the node, and the
//! comparisons are layered so a failure says *where*:
//!
//! 1. **The seam.** The seed the miner uses is the node's own function, so it
//!    cannot drift; asserted anyway, because that is the assumption everything
//!    else rests on.
//! 2. **The CPU mix.** `reference::mix_on_cpu` against `hashimoto_light`,
//!    through `finish`. This isolates the loop from the framing.
//! 3. **The GPU mix.** `hashimoto.wgsl` against `reference::mix_on_cpu` — the
//!    same inputs, so a mismatch is a shader bug and nothing else.
//! 4. **End to end.** GPU mix folded through the node's `finish`, compared
//!    against `hashimoto_light`'s `Proof`.
//!
//! # The GPU tests are skipped, not failed, without an adapter
//!
//! They are `#[cfg(feature = "gpu")]`, and the feature is off by default. A run
//! on a machine with no GPU exercises layers 1 and 2 and reports the rest as
//! absent. A test that failed for want of hardware would be a test people
//! learn to ignore.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::crypto::dag::hashimoto::{finish, hashimoto_light, seed_for};
use custom_l1_node::crypto::dag::{Params, cache::Cache};
use maya_wgpu_miner::reference;

/// Small parameters: a 4 MiB dataset, which fits any adapter and builds in
/// seconds. The algorithm is identical at mainnet sizes; only the dataset is.
const TEST: Params = Params::TESTING;

/// Builds the epoch-0 cache and dataset once per test.
fn fixtures() -> (Cache, custom_l1_node::crypto::dag::dataset::Dataset) {
    let cache = Cache::generate(0, TEST).expect("cache");
    let dataset = custom_l1_node::crypto::dag::dataset::Dataset::generate(&cache).expect("dataset");
    (cache, dataset)
}

fn header() -> [u8; 32] {
    let mut h = [0u8; 32];
    for (index, byte) in h.iter_mut().enumerate() {
        *byte = index as u8;
    }
    h
}

// ---------------------------------------------------------------------------
// Layer 1 and 2: the seam and the CPU mix
// ---------------------------------------------------------------------------

#[test]
fn the_cpu_mix_reproduces_the_nodes_hashimoto() {
    // The reference the GPU is checked against must itself be right, or the
    // parity test below proves only that two wrong things agree.
    let (cache, dataset) = fixtures();
    let header = header();

    for nonce in 0..32u64 {
        let seed = reference::seed_words(&header, nonce);
        let mix = reference::mix_on_cpu(&seed, dataset.pages(), |page| dataset.page(page));
        let proof = finish(&seed_for(&header, nonce), &mix);

        let expected = hashimoto_light(&cache, &header, nonce);
        assert_eq!(proof.mix, expected.mix, "mix diverged at nonce {nonce}");
        assert_eq!(
            proof.result, expected.result,
            "digest diverged at nonce {nonce}"
        );
    }
}

#[test]
fn the_flattened_dataset_addresses_the_same_pages() {
    // The shader indexes a flat word array; the node indexes items. If the two
    // layouts disagreed, every lookup would read the wrong page and the mix
    // would be wrong in a way that looks like a hashing bug.
    let (_cache, dataset) = fixtures();
    let words = reference::flatten(dataset.items());

    for page in [0u32, 1, 7, dataset.pages() - 1] {
        let [first, second] = dataset.page(page);
        let base = page as usize * reference::PAGE_WORDS;
        assert_eq!(&words[base..base + reference::ITEM_WORDS], &first[..]);
        assert_eq!(
            &words[base + reference::ITEM_WORDS..base + reference::PAGE_WORDS],
            &second[..]
        );
    }
}

// ---------------------------------------------------------------------------
// Layer 3 and 4: the GPU
// ---------------------------------------------------------------------------

#[cfg(feature = "gpu")]
mod gpu_tests {
    use super::*;
    use custom_l1_node::crypto::dag::dataset::Dataset;
    use maya_wgpu_miner::gpu::GpuMiner;

    /// Nonces per validation batch. Enough that a bug affecting one lane in a
    /// workgroup, or the boundary between workgroups, is exercised.
    const BATCH: u64 = 256;

    /// A device with the test dataset resident.
    ///
    /// # Run these under nextest, not `cargo test`
    ///
    /// Every test here passes when run alone. Run together under `cargo test`,
    /// the binary **hangs indefinitely**: the harness executes tests on
    /// parallel threads within one process, and creating and dispatching
    /// several wgpu devices concurrently deadlocks on this driver.
    ///
    /// `cargo nextest run` gives each test its own process and the problem does
    /// not arise — which is fortunate, because nextest is this repository's
    /// primary runner (see CLAUDE.md). `cargo test -- --test-threads=1` also
    /// works.
    ///
    /// A shared device behind a `OnceLock<Mutex<..>>` was tried and did **not**
    /// fix it — the deadlock is in device creation and dispatch, not in the
    /// dataset — and it raised an `overflow evaluating ... : Sync` warning that
    /// is slated to become a hard error. It was removed rather than kept with a
    /// comment claiming a fix it did not deliver.
    fn miner(dataset: &Dataset) -> GpuMiner {
        let words = reference::flatten(dataset.items());
        GpuMiner::new(0, &words, dataset.pages(), 64).expect("adapter 0 builds a miner")
    }

    #[test]
    fn the_gpu_mix_matches_the_cpu_mix() {
        // Layer 3: same seeds, same dataset, so any difference is the shader.
        let (_cache, dataset) = fixtures();
        let header = header();
        let miner = miner(&dataset);

        let seeds: Vec<_> = (0..BATCH)
            .map(|nonce| reference::seed_words(&header, nonce))
            .collect();
        let gpu = miner.mixes(&seeds).expect("dispatch");
        assert_eq!(gpu.len(), seeds.len());

        for (nonce, (gpu_mix, seed)) in gpu.iter().zip(&seeds).enumerate() {
            let cpu = reference::mix_on_cpu(seed, dataset.pages(), |page| dataset.page(page));
            assert_eq!(*gpu_mix, cpu, "shader diverged at nonce {nonce}");
        }
    }

    #[test]
    fn the_gpu_digest_matches_the_nodes() {
        // Layer 4: the whole path, against the function a validator runs.
        let (cache, dataset) = fixtures();
        let header = header();
        let miner = miner(&dataset);

        let seeds: Vec<_> = (0..BATCH)
            .map(|nonce| reference::seed_words(&header, nonce))
            .collect();
        let gpu = miner.mixes(&seeds).expect("dispatch");

        for (nonce, gpu_mix) in gpu.iter().enumerate() {
            let nonce = nonce as u64;
            let proof = reference::proof_from_mix(&header, nonce, gpu_mix);
            let expected = hashimoto_light(&cache, &header, nonce);
            assert_eq!(
                proof.result, expected.result,
                "digest diverged at nonce {nonce}"
            );
        }
    }

    #[test]
    fn a_partial_workgroup_is_computed_correctly() {
        // The dispatch is rounded up to whole workgroups and the shader
        // returns early past `nonce_count`. A batch that is not a multiple of
        // the workgroup size is where an off-by-one in that guard shows up —
        // as either a wrong digest or a buffer overrun.
        let (_cache, dataset) = fixtures();
        let header = header();
        let miner = miner(&dataset);

        for count in [1u64, 63, 65, 127] {
            let seeds: Vec<_> = (0..count)
                .map(|nonce| reference::seed_words(&header, nonce))
                .collect();
            let gpu = miner.mixes(&seeds).expect("dispatch");
            assert_eq!(
                gpu.len(),
                count as usize,
                "wrong count for batch of {count}"
            );

            for (nonce, gpu_mix) in gpu.iter().enumerate() {
                let cpu = reference::mix_on_cpu(&seeds[nonce], dataset.pages(), |page| {
                    dataset.page(page)
                });
                assert_eq!(*gpu_mix, cpu, "batch {count}, nonce {nonce}");
            }
        }
    }

    #[test]
    fn an_empty_batch_dispatches_nothing() {
        let (_cache, dataset) = fixtures();
        let miner = miner(&dataset);
        assert!(miner.mixes(&[]).expect("empty batch").is_empty());
    }

    #[test]
    fn adapters_report_their_limits() {
        // The probe the CLI's --list depends on. Asserted rather than assumed:
        // a limit of zero would make `chunks_for` return u64::MAX and the
        // miner would refuse every dataset with a confusing message.
        let found = maya_wgpu_miner::gpu::adapters().expect("at least one adapter");
        assert!(!found.is_empty());
        for adapter in &found {
            assert!(
                adapter.max_storage_binding > 0,
                "{} reports no storage binding limit",
                adapter.name
            );
        }
    }
}
