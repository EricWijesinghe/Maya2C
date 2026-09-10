//! DAG proof-of-work benchmark.
//!
//! Three numbers decide whether this design works, and none of them is a
//! matter of opinion:
//!
//! 1. **Validation cost.** A node checks every block with `hashimoto_light`. If
//!    that is not cheap, running a node is not cheap, and the chain centralises
//!    on whoever can afford to validate. Compare against `--bench argon_blake`,
//!    the rule this replaces, which costs ~25.4 ms.
//! 2. **The light/full ratio.** `hashimoto_light` recomputes what
//!    `hashimoto_full` reads. If light mining were competitive with full
//!    mining, nobody would hold the 4 GiB dataset and the memory requirement —
//!    the whole design — would be optional. Ethash sits around 100×; so should
//!    this.
//! 3. **Epoch cost.** The cache is regenerated once per epoch by every node.
//!    Measured against a 5.21-day budget, it has enormous headroom, but it is
//!    paid at a boundary where a node is also trying to relay blocks.
//!
//! Dataset generation is not benchmarked here. At mainnet sizes it needs 4 GiB
//! resident, which is not something a benchmark suite should allocate on
//! whatever machine runs it; the miner measures it on the hardware that will do
//! it.
//!
//! Run with:
//! ```text
//! cargo bench --bench dag
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use custom_l1_node::crypto::dag::Params;
use custom_l1_node::crypto::dag::cache::Cache;
use custom_l1_node::crypto::dag::dataset::{Dataset, dataset_item};
use custom_l1_node::crypto::dag::hashimoto::{hashimoto_full, hashimoto_light};
use std::hint::black_box;

fn bench_dag(c: &mut Criterion) {
    let header = [0x5Au8; 32];

    // Generated once and shared: at mainnet sizes this is ~0.9 s, and paying it
    // per sample would measure the setup rather than the thing under test.
    let mainnet = Cache::generate(0, Params::MAINNET).expect("cache generation");

    // The full pair at test sizes, so the light/full ratio can be measured
    // against a materialised dataset without allocating 4 GiB here.
    let small = Cache::generate(0, Params::TESTING).expect("cache generation");
    let small_dataset = Dataset::generate(&small).expect("dataset generation");

    let mut group = c.benchmark_group("dag");
    group.sample_size(20);

    // What a validating node pays per block, at consensus sizes. The headline
    // number.
    group.bench_function("hashimoto_light_mainnet", |b| {
        b.iter(|| hashimoto_light(black_box(&mainnet), black_box(&header), 0));
    });

    // One recomputed dataset item: 256 cache reads and two BLAKE3 compressions.
    // A light hash is 128 of these, so this is the unit the number above is
    // built from.
    group.bench_function("dataset_item_mainnet", |b| {
        b.iter(|| dataset_item(black_box(&mainnet), black_box(12_345)));
    });

    // The ratio. Same function, same inputs, one reading memory and one
    // recomputing it.
    group.bench_function("hashimoto_light_small", |b| {
        b.iter(|| hashimoto_light(black_box(&small), black_box(&header), 0));
    });
    group.bench_function("hashimoto_full_small", |b| {
        b.iter(|| hashimoto_full(black_box(&small_dataset), black_box(&header), 0));
    });

    group.finish();

    // Separate group: a single sample is most of a second, so it gets the
    // smallest sample count criterion allows rather than dragging the group
    // above down to it.
    let mut epoch = c.benchmark_group("dag_epoch");
    epoch.sample_size(10);
    epoch.bench_function("cache_generation_mainnet", |b| {
        b.iter(|| Cache::generate(black_box(0), Params::MAINNET).expect("cache generation"));
    });
    epoch.finish();
}

criterion_group!(benches, bench_dag);
criterion_main!(benches);
