//! [SIM] Master Prompt 26 §6: one app suddenly takes 60 % of demand. Other
//! apps' fees and latency must stay within SLO.
//!
//! Model: 10 apps, 8 transactions per app per block at baseline; from block
//! 100, app 0 wants 135 per block (135 / 225 = 60 % of demand); blocks hold
//! 200. A sender bids 2–5× the current price of the app it touches (viral
//! users 2–20×) and gives up after 30 blocks. Run twice: local fee markets
//! with a per-app cap, and a single global fee market as the control. SLO for
//! the other nine apps: p99 inclusion within 2 blocks, median fee within 2×
//! their pre-spike median.

#![allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::collections::BTreeMap;

use maya_lanes::{Fees, Params, Tx, build};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

struct Outcome {
    before_fee: u64,
    after_fee: u64,
    p99_latency: u64,
    dropped: usize,
    viral_included: usize,
}

fn percentile(v: &mut [u64], p: usize) -> u64 {
    v.sort_unstable();
    v.get(v.len().saturating_sub(1).min(v.len() * p / 100))
        .copied()
        .unwrap_or(0)
}

fn run(params: Params) -> Outcome {
    let mut fees = Fees::new(params);
    let mut rng = Rng(26);
    let mut pool: Vec<Tx> = Vec::new();
    let (mut before, mut after, mut latency) = (Vec::new(), Vec::new(), Vec::new());
    let (mut dropped, mut viral_included) = (0, 0);
    for h in 0..400u64 {
        for app in 0..10u32 {
            let demand = if app == 0 && h >= 100 { 135 } else { 8 };
            for _ in 0..demand {
                let top = if app == 0 && h >= 100 { 19 } else { 4 };
                let m = 2 + rng.next() % top;
                pool.push(Tx {
                    app,
                    max_fee: fees.price(app).saturating_mul(m),
                    lane: None,
                    arrived: h,
                });
            }
        }
        let prices: BTreeMap<u32, u64> = (0..10).map(|a| (a, fees.price(a))).collect();
        let block = build(&mut pool, &[], &fees);
        let mut usage = BTreeMap::new();
        for tx in &block {
            *usage.entry(tx.app).or_insert(0) += 1;
            if tx.app == 0 {
                viral_included += 1;
            } else if h < 100 {
                before.push(prices[&tx.app]);
            } else {
                after.push(prices[&tx.app]);
                latency.push(h - tx.arrived);
            }
        }
        let n = pool.len();
        pool.retain(|t| h - t.arrived < 30);
        dropped += n - pool.len();
        fees.update(&usage);
    }
    Outcome {
        before_fee: percentile(&mut before, 50),
        after_fee: percentile(&mut after, 50),
        p99_latency: percentile(&mut latency, 99),
        dropped,
        viral_included,
    }
}

#[test]
fn a_viral_app_raises_its_own_fees_not_everyone_elses() {
    // Global target 150 sits *below* cap (80) + steady demand (72) = 152: the
    // EIP-1559 "+1 at least" step then lifts the global fee every block. It is
    // kept as the configuration that fails, because that is the design rule
    // this simulation found: global target > per-app cap + steady demand.
    let tight = run(Params {
        capacity: 200,
        app_target: 10,
        global_target: 150,
        denom: 8,
        floor: 1,
        app_cap: 80,
    });
    let local = run(Params {
        capacity: 200,
        app_target: 10,
        global_target: 160,
        denom: 8,
        floor: 1,
        app_cap: 80,
    });
    let global = run(Params {
        capacity: 200,
        app_target: u64::MAX / 4,
        global_target: 160,
        denom: 8,
        floor: 1,
        app_cap: 200,
    });
    for (name, o) in [
        (
            "local + cap, global target 150 (below cap + demand)",
            &tight,
        ),
        ("local + cap, global target 160", &local),
        ("global fee only, target 160 (control)", &global),
    ] {
        println!(
            "[SIM] {name}: other apps' median fee {} -> {} ({:.1}x), p99 inclusion {} blocks; {} viral txs included; {} txs dropped after 30 blocks",
            o.before_fee,
            o.after_fee,
            o.after_fee as f64 / o.before_fee.max(1) as f64,
            o.p99_latency,
            o.viral_included,
            o.dropped
        );
    }
    assert!(
        tight.after_fee > 2 * tight.before_fee,
        "the tight configuration is expected to breach the SLO"
    );
    assert!(
        local.p99_latency <= 2,
        "other apps must be included within 2 blocks"
    );
    assert!(
        local.after_fee <= 2 * local.before_fee,
        "other apps' fees must stay within 2x"
    );
    assert!(
        global.after_fee > 2 * global.before_fee || global.p99_latency > 2,
        "the control must show the problem the design solves"
    );
}
