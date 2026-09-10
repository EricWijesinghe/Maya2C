//! Trading engine throughput.
//!
//! Three numbers decide whether a native DEX belongs in a block at all:
//!
//! 1. **Matching cost per fill.** Every node redoes the block's matching pass.
//!    `MAX_FILLS_PER_BLOCK` is 1024, so a fill that costs a microsecond is a
//!    millisecond of block validation and a fill that costs a hundred is a
//!    tenth of a second — the difference between a bound that is generous and a
//!    bound that is the block time.
//! 2. **Batch clearing cost.** The clearing loop drops intents that miss their
//!    slippage bound and re-prices, which is quadratic in the worst case. The
//!    worst case is one removal per round, and it is attacker-reachable: fill
//!    a batch with bounds that fail in cascade. Measured here directly.
//! 3. **Book reconstruction.** A book is rebuilt from storage each time it is
//!    crossed. The order keys are laid out so no sort is needed; this measures
//!    what is left.
//!
//! What is *not* measured here is storage. These are the pure-arithmetic costs,
//! which is the half that is a property of the design rather than of RocksDB's
//! mood. Whole-block execution is covered by `tests/dex_concurrency_tests.rs`
//! at realistic widths.
//!
//! Run with:
//! ```text
//! cargo bench --bench dex_matching
//! ```

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use maya_dex::amm::Pool;
use maya_dex::batch::{MAX_BATCH_INTENTS, SwapIntent, clear_batch};
use maya_dex::book::{Book, Order, Side};
use maya_dex::fees::FeeSchedule;
use maya_dex::matching::{MatchLimits, match_book};
use maya_dex::types::{Direction, PRICE_SCALE};
use std::hint::black_box;

/// A pool deep enough that no benchmark trade exhausts it.
fn deep_pool() -> Pool {
    Pool::empty(FeeSchedule::standard())
        .add_liquidity(1_000_000_000, 1_000_000_000)
        .expect("seed")
        .pool
}

/// A book of `depth` orders per side, crossing in the middle.
///
/// Prices fan out from the midpoint so the pass walks the queue rather than
/// filling everything against one level, which is the shape that actually costs
/// something.
fn crossed_book(depth: u32) -> Book {
    let mid = PRICE_SCALE as u64;
    let mut book = Book::new();

    for index in 0..depth {
        let mut id = [0u8; 32];
        id[0..4].copy_from_slice(&index.to_le_bytes());
        id[31] = 1;
        book.insert(Order {
            id,
            owner: [1u8; 32],
            side: Side::Ask,
            price: mid - u64::from(index),
            amount: 1_000,
            remaining: 1_000,
            sequence: u64::from(index) * 2,
            expiry: 0,
        })
        .expect("ask");

        let mut id = [0u8; 32];
        id[0..4].copy_from_slice(&index.to_le_bytes());
        id[31] = 2;
        book.insert(Order {
            id,
            owner: [2u8; 32],
            side: Side::Bid,
            price: mid + u64::from(index),
            amount: 1_000,
            remaining: 1_000,
            sequence: u64::from(index) * 2 + 1,
            expiry: 0,
        })
        .expect("bid");
    }

    book
}

fn bench_matching(c: &mut Criterion) {
    let fees = FeeSchedule::standard();
    let mut group = c.benchmark_group("matching");

    for depth in [16u32, 128, 512] {
        let book = crossed_book(depth);
        group.bench_function(format!("cross_{depth}_per_side"), |b| {
            b.iter(|| {
                let outcome = match_book(
                    black_box(&book),
                    fees,
                    MatchLimits {
                        max_fills: u32::MAX,
                        height: 1,
                    },
                )
                .expect("match");
                black_box(outcome.fills.len())
            });
        });
    }

    // Reconstruction, separated from crossing: the claim is that the storage
    // key ordering makes this a series of inserts into an already-sorted map
    // rather than a sort.
    let orders = crossed_book(512).orders();
    group.bench_function("rebuild_1024", |b| {
        b.iter_batched(
            Book::new,
            |mut book| {
                for order in &orders {
                    book.insert(*order).expect("insert");
                }
                black_box(book.len())
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_batch(c: &mut Criterion) {
    let pool = deep_pool();
    let mut group = c.benchmark_group("batch");

    // The ordinary case: every intent clears on the first pass.
    for count in [8usize, 64, MAX_BATCH_INTENTS] {
        let intents: Vec<SwapIntent> = (0..count)
            .map(|index| SwapIntent {
                id: [index as u8; 32],
                trader: [index as u8; 32],
                direction: if index % 2 == 0 {
                    Direction::BaseToQuote
                } else {
                    Direction::QuoteToBase
                },
                amount_in: 100_000 + index as u64 * 977,
                min_out: 0,
            })
            .collect();

        group.bench_function(format!("clear_{count}"), |b| {
            b.iter(|| {
                let outcome = clear_batch(black_box(&pool), black_box(&intents)).expect("clear");
                black_box(outcome.price)
            });
        });
    }

    // The adversarial case: every intent's bound fails, so the fixed-point loop
    // removes one per round and re-prices. This is the quadratic path, and the
    // number it produces is the one that justifies `MAX_BATCH_INTENTS`.
    let hostile: Vec<SwapIntent> = (0..MAX_BATCH_INTENTS)
        .map(|index| SwapIntent {
            id: [index as u8; 32],
            trader: [index as u8; 32],
            direction: Direction::BaseToQuote,
            amount_in: 100_000 + index as u64,
            min_out: u64::MAX,
        })
        .collect();

    group.bench_function("clear_full_cascade", |b| {
        b.iter(|| {
            let outcome = clear_batch(black_box(&pool), black_box(&hostile)).expect("clear");
            black_box(outcome.skipped.len())
        });
    });

    group.finish();
}

fn bench_curve(c: &mut Criterion) {
    // The single swap, for scale. Everything above is some number of these plus
    // bookkeeping, so a reader who wants to know where the time goes needs this
    // one first.
    let pool = deep_pool();
    c.bench_function("swap_exact_in", |b| {
        b.iter(|| {
            let outcome = pool
                .swap_exact_in(black_box(Direction::BaseToQuote), black_box(1_000_000))
                .expect("swap");
            black_box(outcome.amount_out)
        });
    });
}

criterion_group!(benches, bench_curve, bench_matching, bench_batch);
criterion_main!(benches);
