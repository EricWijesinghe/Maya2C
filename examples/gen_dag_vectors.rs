//! Regenerates the frozen DAG test vectors at `tests/fixtures/dag_vectors.json`.
//!
//! ```text
//! cargo run --release --example gen_dag_vectors > tests/fixtures/dag_vectors.json
//! ```
//!
//! ## What the file is for
//!
//! The vectors pin the *bytes* of the proof-of-work, not merely its internal
//! consistency. `tests/dag_tests.rs` checks the node against them and
//! `cuda-miner/tests/dag_parity.rs` checks the GPU miner's host reference
//! against the same numbers, so a change to either implementation that the
//! other does not make is a failing test rather than a chain split.
//!
//! **Regenerating the file is a hard fork.** If a vector no longer matches, the
//! answer is almost always that the code changed and should not have. The one
//! legitimate reason to re-run this is a deliberate, documented consensus
//! change — the same standard `argon_blake_matches_the_frozen_known_answer` in
//! `tests/crypto_tests.rs` holds ArgonBlake to.
//!
//! Mainnet vectors are included and cost one 64 MiB cache to check — never the
//! 4 GiB dataset, because a dataset item is computable from the cache alone.
//! That is exactly the asymmetry the design is built on, so the vectors are
//! also a demonstration of it.

use custom_l1_node::crypto::dag::cache::Cache;
use custom_l1_node::crypto::dag::dataset::{Dataset, dataset_item};
use custom_l1_node::crypto::dag::hashimoto::{hashimoto_full, hashimoto_light};
use custom_l1_node::crypto::dag::{Params, epoch_seed, item_to_le_bytes};
use serde_json::{Value, json};

/// Headers the hashimoto vectors are taken over.
const HEADERS: [[u8; 32]; 2] = [[0xA5; 32], [0x00; 32]];

/// Nonces the hashimoto vectors are taken over. Includes one above 2³² so a
/// 32-bit truncation somewhere shows up as a mismatch rather than as luck.
const NONCES: [u64; 4] = [0, 1, 7, 4_294_967_297];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cases = Vec::new();
    for epoch in 0..2u64 {
        cases.push(testing_case(epoch)?);
    }

    let document = json!({
        "note": "Frozen DAG proof-of-work vectors. Regenerating this file is a \
                 consensus change; see examples/gen_dag_vectors.rs.",
        "testing_params": describe(Params::TESTING),
        "mainnet_params": describe(Params::MAINNET),
        "epoch_seeds": (0..6).map(|epoch| hex::encode(epoch_seed(epoch))).collect::<Vec<_>>(),
        "testing_cases": cases,
        "mainnet_case": mainnet_case()?,
    });

    println!("{}", serde_json::to_string_pretty(&document)?);
    Ok(())
}

/// The parameter set, so a reader can tell what the vectors were taken at.
fn describe(params: Params) -> Value {
    json!({
        "epoch_length": params.epoch_length,
        "cache_bytes": params.cache_bytes,
        "dataset_bytes": params.dataset_bytes,
        "cache_items": params.cache_items(),
        "dataset_pages": params.dataset_pages(),
    })
}

/// Vectors at [`Params::TESTING`], including whole-dataset checks.
fn testing_case(epoch: u64) -> Result<Value, Box<dyn std::error::Error>> {
    let cache = Cache::generate(epoch, Params::TESTING)?;
    let dataset = Dataset::generate(&cache)?;
    let last = dataset.len() as u32 - 1;

    Ok(json!({
        "epoch": epoch,
        "cache_item_first": hex::encode(item_to_le_bytes(cache.item(0))),
        "cache_item_last": hex::encode(item_to_le_bytes(cache.item(cache.len() - 1))),
        "dataset_items": sampled_items(&cache, &[0, 1, 17, 12_345, last]),
        // A digest over every item, so the vectors pin the whole 4 MiB rather
        // than five spot checks that a subtly wrong generator could still pass.
        "dataset_digest": hex::encode(digest_of(&dataset)),
        "hashimoto": proofs(&cache, Some(&dataset)),
    }))
}

/// Vectors at [`Params::MAINNET`], from the cache only.
fn mainnet_case() -> Result<Value, Box<dyn std::error::Error>> {
    let cache = Cache::generate(0, Params::MAINNET)?;
    let last = Params::MAINNET.dataset_items() - 1;
    Ok(json!({
        "epoch": 0,
        "cache_item_first": hex::encode(item_to_le_bytes(cache.item(0))),
        "cache_item_last": hex::encode(item_to_le_bytes(cache.item(cache.len() - 1))),
        "dataset_items": sampled_items(&cache, &[0, 1, 17, 12_345, last]),
        "hashimoto": proofs(&cache, None),
    }))
}

/// Named dataset items, recomputed from the cache.
fn sampled_items(cache: &Cache, indices: &[u32]) -> Value {
    let entries: Vec<Value> = indices
        .iter()
        .map(|index| {
            json!({
                "index": index,
                "item": hex::encode(item_to_le_bytes(&dataset_item(cache, *index))),
            })
        })
        .collect();
    Value::Array(entries)
}

/// Hashimoto proofs over every header and nonce.
///
/// When a dataset is supplied the full path is evaluated too and its output is
/// asserted equal to the light path before anything is written — a vector file
/// that recorded a disagreement would be worse than no vector file at all.
fn proofs(cache: &Cache, dataset: Option<&Dataset>) -> Value {
    let mut entries = Vec::new();
    for header in HEADERS {
        for nonce in NONCES {
            let light = hashimoto_light(cache, &header, nonce);
            if let Some(dataset) = dataset {
                let full = hashimoto_full(dataset, &header, nonce);
                assert_eq!(light, full, "light and full disagree; refusing to emit");
            }
            entries.push(json!({
                "header": hex::encode(header),
                "nonce": nonce,
                "mix": hex::encode(light.mix),
                "result": hex::encode(light.result),
            }));
        }
    }
    Value::Array(entries)
}

/// BLAKE3 over every dataset item, in order.
fn digest_of(dataset: &Dataset) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for item in dataset.items() {
        hasher.update(&item_to_le_bytes(item));
    }
    *hasher.finalize().as_bytes()
}
