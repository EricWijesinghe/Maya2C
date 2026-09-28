//! Master Prompt 6 §8's scale tests for the energy module: 50,000 battery
//! events in one epoch, 1,000,000 micro-power transfers netted in parallel,
//! and a 500 MW surge allocated to compute load (SIM).

#![allow(clippy::unwrap_used)]

use std::time::Instant;

use maya_energy::market::{BatteryEvent, PowerTransfer, net_transfers, settle_epoch};
use maya_energy::surge::{Miner, absorb};

fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn fifty_thousand_battery_events_settle_in_one_epoch() {
    let mut seed = 0xBA77_u64;
    let events: Vec<BatteryEvent> = (0..50_000u32)
        .map(|i| {
            let mut account = [0u8; 32];
            account[..4].copy_from_slice(&(i % 20_000).to_le_bytes());
            let wh = i64::try_from(next(&mut seed) % 20_000).unwrap() - 10_000;
            BatteryEvent { account, wh }
        })
        .collect();
    let started = Instant::now();
    let (net, operator) = settle_epoch(&events, 120).unwrap();
    let elapsed = started.elapsed();
    let owners: i128 = net.values().sum();
    assert_eq!(owners + operator, 0, "every payment has a counterparty");
    println!(
        "50000 battery events settled in one epoch across {} accounts in {elapsed:?}; operator net {operator}",
        net.len()
    );
}

#[test]
fn a_million_micro_power_transfers_net_the_same_however_split() {
    const PARTIES: u32 = 10_000;
    let mut seed = 0x5EED_u64;
    let transfers: Vec<PowerTransfer> = (0..1_000_000)
        .map(|_| {
            let (a, b) = (next(&mut seed), next(&mut seed));
            PowerTransfer {
                from: u32::try_from(a % u64::from(PARTIES)).unwrap(),
                to: u32::try_from(b % u64::from(PARTIES)).unwrap(),
                wh: u32::try_from(a % 50).unwrap() + 1,
                price: u32::try_from(b % 30).unwrap() + 1,
            }
        })
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
    let started = Instant::now();
    let parallel = net_transfers(&transfers, PARTIES as usize, threads);
    let elapsed = started.elapsed();
    let serial = net_transfers(&transfers, PARTIES as usize, 1);
    assert_eq!(parallel, serial, "the split does not change the result");
    assert_eq!(parallel.iter().sum::<i64>(), 0, "value is conserved");
    println!("1000000 micro-power transfers netted on {threads} threads in {elapsed:?}");
}

#[test]
fn a_500_mw_surge_is_allocated_to_compute_load_in_under_10_ms() {
    // SIM: 20,000 miners with 10 to 60 kW of headroom each.
    let mut seed = 0x0501_u64;
    let miners: Vec<Miner> = (0..20_000u32)
        .map(|id| Miner {
            id,
            headroom_kw: 10 + next(&mut seed) % 51,
        })
        .collect();
    let surge_kw = 500_000;
    let started = Instant::now();
    let plan = absorb(surge_kw, &miners);
    let elapsed = started.elapsed();
    assert_eq!(plan.unabsorbed_kw, 0);
    assert_eq!(plan.loads.iter().map(|(_, kw)| kw).sum::<u64>(), surge_kw);
    println!(
        "SIM: 500 MW surge allocated to {} of 20000 miners in {elapsed:?} (the decision only; ramping and grid physics are not modelled)",
        plan.loads.len()
    );
    // The brief's 10 ms is a target for an optimised build; a debug build
    // under a loaded test run only reports its time.
    if !cfg!(debug_assertions) {
        assert!(elapsed.as_millis() < 10, "the decision took {elapsed:?}");
    }
    // More surplus than headroom is reported, not hidden.
    assert!(absorb(u64::MAX / 2, &miners[..10]).unabsorbed_kw > 0);
}
