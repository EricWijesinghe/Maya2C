//! The miner's DAG against the node's, and both against the frozen vectors.
//!
//! ## What is being defended
//!
//! `src/dag.rs` is a second implementation of a consensus rule. A second
//! implementation of a consensus rule is a chain split waiting for a
//! disagreement, and the disagreement would surface as blocks the network
//! rejects for no visible reason — after the electricity has been spent.
//!
//! So the two are compared directly, on the same inputs, at every level the
//! computation has: the epoch seed, the cache, individual dataset items, the
//! whole dataset, and hashimoto itself. Then both are compared against
//! `tests/fixtures/dag_vectors.json`, which is what stops the two from drifting
//! *together* — a change made in both places at once would pass every parity
//! assertion and still fork the chain.
//!
//! ## What this cannot check
//!
//! Whether `kernels/dag.cu` is a faithful transliteration of `src/dag.rs`. That
//! needs a GPU. What it can and does establish is that everything the kernel is
//! transliterated *from* — including the hand-rolled BLAKE3 in
//! `src/blake3_ref.rs` — is byte-exact against the node.

use custom_l1_node::core::BlockHeader;
use custom_l1_node::crypto::dag::cache::Cache as NodeCache;
use custom_l1_node::crypto::dag::dataset::{Dataset as NodeDataset, dataset_item as node_item};
use custom_l1_node::crypto::dag::hashimoto::{
    hashimoto_full as node_full, hashimoto_light as node_light,
};
use custom_l1_node::crypto::dag::{Params as NodeParams, epoch_seed as node_epoch_seed};
use maya_cuda_miner::dag::{
    ITEM_WORDS, Keys, Params, dataset_item, epoch_seed, generate_cache, generate_dataset,
    hashimoto_full, hashimoto_light, item_to_le_bytes, pow_seed,
};
use serde_json::Value;

/// Headers the vectors are taken over.
const HEADERS: [[u8; 32]; 2] = [[0xA5; 32], [0x00; 32]];

/// Nonces the vectors are taken over.
const NONCES: [u64; 4] = [0, 1, 7, 4_294_967_297];

fn vectors() -> Value {
    // CARGO_MANIFEST_DIR is <repo>/hal/cuda-miner, and the vectors live with
    // the node's own tests. It was <repo>/cuda-miner until the phase B move —
    // see docs/adr/ADR-001-workspace-layout.md.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/node/tests/fixtures/dag_vectors.json"
    );
    let text = std::fs::read_to_string(path).expect("the frozen vectors must be readable");
    serde_json::from_str(&text).expect("the frozen vectors must be valid JSON")
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("the vectors are missing `{key}`"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string"))
}

/// The miner's item at `index`, as hex.
fn miner_item_hex(cache: &[u32], index: u32, keys: &Keys) -> String {
    hex::encode(item_to_le_bytes(&dataset_item(cache, index, keys)))
}

// ---------------------------------------------------------------------------
// against the node
// ---------------------------------------------------------------------------

#[test]
fn the_parameter_sets_agree() {
    // Everything below compares bytes derived from these counts. If the counts
    // themselves diverged, the comparisons would be of different-sized objects
    // and the failure would be confusing rather than pointed.
    for (miner, node) in [
        (Params::MAINNET, NodeParams::MAINNET),
        (Params::TESTING, NodeParams::TESTING),
    ] {
        assert_eq!(miner.epoch_length, node.epoch_length);
        assert_eq!(miner.cache_bytes, node.cache_bytes);
        assert_eq!(miner.dataset_bytes, node.dataset_bytes);
        assert_eq!(miner.cache_items(), node.cache_items());
        assert_eq!(miner.dataset_pages(), node.dataset_pages());
        assert_eq!(miner.dataset_items(), node.dataset_items());
        assert_eq!(miner.epoch_of(31_337), node.epoch_of(31_337));
    }
}

#[test]
fn the_epoch_seed_chain_agrees() {
    for epoch in 0..6u64 {
        assert_eq!(
            epoch_seed(epoch),
            node_epoch_seed(epoch),
            "epoch {epoch} seed diverged"
        );
    }
}

#[test]
fn the_cache_agrees_item_for_item() {
    // The cache is the root of everything else: a single wrong item makes every
    // dataset item that reads it wrong, and 256 of them read each item.
    let keys = Keys::derive();
    let miner = generate_cache(0, Params::TESTING, &keys);
    let node = NodeCache::generate(0, NodeParams::TESTING).expect("cache generation");

    assert_eq!(miner.len(), node.len() * ITEM_WORDS);
    for index in 0..node.len() {
        let mined = &miner[index * ITEM_WORDS..(index + 1) * ITEM_WORDS];
        assert_eq!(mined, node.item(index), "cache item {index} diverged");
    }
}

#[test]
fn the_dataset_agrees_item_for_item() {
    let keys = Keys::derive();
    let cache = generate_cache(0, Params::TESTING, &keys);
    let node_cache = NodeCache::generate(0, NodeParams::TESTING).expect("cache generation");
    let node_dataset = NodeDataset::generate(&node_cache).expect("dataset generation");

    // The whole dataset, not a sample: this is the object the kernel will
    // materialise on the device, and any region of it can be read.
    let miner_dataset = generate_dataset(&cache, Params::TESTING, &keys);
    assert_eq!(miner_dataset.len(), node_dataset.len() * ITEM_WORDS);

    for index in 0..node_dataset.len() as u32 {
        let base = index as usize * ITEM_WORDS;
        assert_eq!(
            &miner_dataset[base..base + ITEM_WORDS],
            node_dataset.item(index),
            "dataset item {index} diverged"
        );
        // And the on-demand path, which is what a validator runs and what the
        // miner uses to check a solution before submitting it.
        assert_eq!(
            dataset_item(&cache, index, &keys),
            *node_item(&node_cache, index).as_ref(),
            "recomputed item {index} diverged"
        );
    }
}

#[test]
fn hashimoto_agrees_on_both_paths() {
    let keys = Keys::derive();
    let cache = generate_cache(0, Params::TESTING, &keys);
    let dataset = generate_dataset(&cache, Params::TESTING, &keys);
    let node_cache = NodeCache::generate(0, NodeParams::TESTING).expect("cache generation");
    let node_dataset = NodeDataset::generate(&node_cache).expect("dataset generation");
    let pages = Params::TESTING.dataset_pages();

    for header in HEADERS {
        for nonce in NONCES {
            let node = node_light(&node_cache, &header, nonce);
            assert_eq!(node, node_full(&node_dataset, &header, nonce));

            let light = hashimoto_light(&cache, pages, &header, nonce, &keys);
            let full = hashimoto_full(&dataset, pages, &header, nonce, &keys);

            assert_eq!(light, full, "the miner's own two paths diverged");
            assert_eq!(light.mix, node.mix, "the mix diverged at nonce {nonce}");
            assert_eq!(
                light.result, node.result,
                "the digest diverged at nonce {nonce}"
            );
        }
    }
}

#[test]
fn the_search_seed_agrees() {
    // The miner is handed raw header bytes by a pool and has to arrive at the
    // same 32 bytes the node derives from a decoded header — including which
    // eight bytes are the nonce.
    let header = BlockHeader {
        prev_hash: [3u8; 32],
        state_root: [4u8; 32],
        timestamp: 1_700_000_000,
        nonce: 0xDEAD_BEEF_CAFE_F00D,
        difficulty_target: [0x0F; 32],
        tx_root: [0; 32],
    };

    assert_eq!(pow_seed(&header.serialize()), header.pow_seed());

    // And the nonce is genuinely excluded from it on both sides.
    let mut other = header.clone();
    other.nonce = 1;
    assert_eq!(other.pow_seed(), header.pow_seed());
}

// ---------------------------------------------------------------------------
// against the frozen vectors
// ---------------------------------------------------------------------------

#[test]
fn the_miner_matches_the_frozen_testing_vectors() {
    let vectors = vectors();
    let keys = Keys::derive();
    let pages = Params::TESTING.dataset_pages();

    for case in field(&vectors, "testing_cases")
        .as_array()
        .expect("testing_cases must be an array")
    {
        let epoch = field(case, "epoch")
            .as_u64()
            .expect("epoch must be a number");
        let cache = generate_cache(epoch, Params::TESTING, &keys);

        assert_eq!(
            hex::encode(item_to_le_bytes(&first_item(&cache))),
            text(case, "cache_item_first"),
            "epoch {epoch}: the miner's cache diverged from the vectors"
        );

        check_items(case, &cache, &keys, epoch);
        check_hashimoto(case, &cache, pages, &keys, epoch);
    }
}

#[test]
fn the_miner_matches_the_frozen_mainnet_vectors() {
    // The vectors that govern the chain this miner will actually mine, checked
    // in the miner's own implementation, with no node involved. A 64 MiB cache
    // and no dataset — the same asymmetry a validator relies on.
    let vectors = vectors();
    let case = field(&vectors, "mainnet_case");
    let keys = Keys::derive();
    let cache = generate_cache(0, Params::MAINNET, &keys);

    assert_eq!(
        hex::encode(item_to_le_bytes(&first_item(&cache))),
        text(case, "cache_item_first"),
        "the miner's mainnet cache diverged from the vectors"
    );

    check_items(case, &cache, &keys, 0);
    check_hashimoto(case, &cache, Params::MAINNET.dataset_pages(), &keys, 0);
}

fn first_item(cache: &[u32]) -> [u32; ITEM_WORDS] {
    let mut item = [0u32; ITEM_WORDS];
    item.copy_from_slice(&cache[..ITEM_WORDS]);
    item
}

fn check_items(case: &Value, cache: &[u32], keys: &Keys, epoch: u64) {
    for entry in field(case, "dataset_items")
        .as_array()
        .expect("dataset_items must be an array")
    {
        let index = field(entry, "index")
            .as_u64()
            .expect("index must be a number");
        let index = u32::try_from(index).expect("index must fit a u32");
        assert_eq!(
            miner_item_hex(cache, index, keys),
            text(entry, "item"),
            "epoch {epoch}: dataset item {index} diverged from the vectors"
        );
    }
}

// ---------------------------------------------------------------------------
// against the kernel
// ---------------------------------------------------------------------------
//
// Compiled only with `--features cuda`, and skipped at runtime when no device
// is present, so a developer with the toolkit but no card still gets a green
// suite. These are the assertions that close the last gap: everything above
// proves the Rust the kernel was transliterated from, and these prove the
// transliteration.

#[cfg(feature = "cuda")]
mod kernel {
    use super::*;
    use maya_cuda_miner::gpu::{GpuDag, device_count};

    /// A card is needed, not merely a toolkit.
    fn device() -> Option<i32> {
        (device_count() > 0).then_some(0)
    }

    #[test]
    fn the_kernel_generates_the_same_dataset_as_the_cpu() {
        let Some(device) = device() else {
            eprintln!("no CUDA device; skipping");
            return;
        };

        let keys = Keys::derive();
        let cache = generate_cache(0, Params::TESTING, &keys);
        let expected = generate_dataset(&cache, Params::TESTING, &keys);

        let gpu = GpuDag::create(device, 0, Params::TESTING, &cache)
            .expect("the test dataset is a few megabytes and must fit");

        // The whole dataset, item for item. At test sizes this is a few
        // megabytes over PCIe; at mainnet sizes it would be pointless, which is
        // why the parameters are a value.
        let items = Params::TESTING.dataset_items();
        let actual = gpu.read_items(0, items).expect("readback must succeed");

        for (index, item) in actual.iter().enumerate() {
            let base = index * ITEM_WORDS;
            assert_eq!(
                item.as_slice(),
                &expected[base..base + ITEM_WORDS],
                "the kernel and the CPU disagree on dataset item {index}"
            );
        }
        assert_eq!(actual.len(), items as usize, "the readback was short");
    }

    #[test]
    fn the_kernel_finds_only_nonces_the_cpu_agrees_are_solutions() {
        let Some(device) = device() else {
            eprintln!("no CUDA device; skipping");
            return;
        };

        let keys = Keys::derive();
        let cache = generate_cache(0, Params::TESTING, &keys);
        let pages = Params::TESTING.dataset_pages();
        let gpu = GpuDag::create(device, 0, Params::TESTING, &cache).expect("dataset");

        let seed = [0xA5u8; 32];
        // One solution in 256: found in the first batch, and rare enough that
        // finding one at all is evidence the digest was computed and compared
        // rather than defaulted.
        let mut target = [0xFFu8; 32];
        target[0] = 0x00;

        let found = gpu
            .search(&seed, 0, 65_536, &target)
            .expect("search must succeed")
            .expect("a solution must exist within 65,536 nonces");

        // The kernel's claim, checked by the CPU reference — which is itself
        // checked against the node above. A false positive here would be a
        // miner submitting invalid blocks.
        let proof = hashimoto_light(&cache, pages, &seed, found, &keys);
        assert!(
            proof.result <= target,
            "the kernel reported nonce {found} as a solution; the CPU disagrees"
        );
    }

    #[test]
    fn the_kernel_and_the_cpu_agree_on_a_nonce_that_is_not_a_solution() {
        // The other direction: a target nothing can satisfy must produce no
        // claim at all. A kernel that reported success unconditionally would
        // pass the test above and fail this one.
        let Some(device) = device() else {
            eprintln!("no CUDA device; skipping");
            return;
        };

        let keys = Keys::derive();
        let cache = generate_cache(0, Params::TESTING, &keys);
        let gpu = GpuDag::create(device, 0, Params::TESTING, &cache).expect("dataset");

        let impossible = [0u8; 32];
        assert_eq!(
            gpu.search(&[0x11u8; 32], 0, 4_096, &impossible)
                .expect("search must succeed"),
            None
        );
    }
}

fn check_hashimoto(case: &Value, cache: &[u32], pages: u32, keys: &Keys, epoch: u64) {
    for entry in field(case, "hashimoto")
        .as_array()
        .expect("hashimoto must be an array")
    {
        let header: [u8; 32] = hex::decode(text(entry, "header"))
            .expect("header must be hex")
            .try_into()
            .expect("header must be 32 bytes");
        let nonce = field(entry, "nonce")
            .as_u64()
            .expect("nonce must be a number");

        let proof = hashimoto_light(cache, pages, &header, nonce, keys);
        assert_eq!(
            hex::encode(proof.mix),
            text(entry, "mix"),
            "epoch {epoch}: mix diverged from the vectors at nonce {nonce}"
        );
        assert_eq!(
            hex::encode(proof.result),
            text(entry, "result"),
            "epoch {epoch}: digest diverged from the vectors at nonce {nonce}"
        );
    }
}
