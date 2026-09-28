//! Machine-to-machine barter (Master Prompt 6 §2): 10,000 simulated devices
//! trading compute, bandwidth, storage and power under SLAs, with faults
//! slashed, settled as one netted batch that conserves value.

#![allow(clippy::unwrap_used)]

use l2_flash::barter::{BarterError, Market, Resource, Sla};

const DEVICES: u32 = 10_000;
const TICKS: u64 = 100;
const FUNDS: u64 = 1_000_000;

fn device(i: u32) -> [u8; 32] {
    let mut a = [0u8; 32];
    a[..4].copy_from_slice(&i.to_le_bytes());
    a
}

fn resource(i: u32) -> Resource {
    [
        Resource::Compute,
        Resource::Bandwidth,
        Resource::Storage,
        Resource::Power,
    ][usize::try_from(i % 4).unwrap()]
}

#[test]
fn ten_thousand_devices_barter_and_settle_in_one_batch() {
    let mut market = Market::default();
    for i in 0..DEVICES {
        market.fund(device(i), FUNDS);
    }
    let total = u64::from(DEVICES) * FUNDS;
    // A ring: each device serves the next one.
    let slas: Vec<usize> = (0..DEVICES)
        .map(|i| {
            market
                .open(Sla {
                    provider: device(i),
                    consumer: device((i + 1) % DEVICES),
                    resource: resource(i),
                    price_per_unit: 2 + u64::from(i % 5),
                    committed_per_tick: 10,
                    floor_bps: 9_000,
                    slash_per_breach: 50,
                    collateral: 1_000,
                })
                .unwrap()
        })
        .collect();

    // Deterministic faults: about 1 delivery in 100 falls short.
    let started = std::time::Instant::now();
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut breaches = 0u64;
    for _ in 0..TICKS {
        for id in &slas {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let delivered = if seed.is_multiple_of(100) { 5 } else { 10 };
            breaches += u64::from(market.tick(*id, delivered).unwrap());
        }
    }
    let elapsed = started.elapsed();
    let batch = market.settle().unwrap();
    let per_second = f64::from(u32::try_from(batch.updates).unwrap()) / elapsed.as_secs_f64();
    println!(
        "barter (simulated devices): {DEVICES} devices x {TICKS} ticks = {} updates, {breaches} breaches slashed, in {elapsed:?} ({per_second:.0} updates/s, accounting only); one settlement of {} deltas",
        batch.updates,
        batch.deltas.len()
    );
    assert_eq!(batch.updates, u64::from(DEVICES) * TICKS);
    assert_eq!(batch.breaches, breaches);
    assert!(breaches > 0);

    // Value is conserved: balances plus collateral in escrow equal what was funded.
    let held = market.collateral_held();
    let balances: u64 = (0..DEVICES).map(|i| market.balance(&device(i))).sum();
    assert_eq!(balances + held, total);
    for id in slas {
        market.close(id).unwrap();
    }
    assert_eq!(market.collateral_held(), 0);
    let returned = market.settle().unwrap();
    assert_eq!(
        returned.deltas.iter().map(|(_, d)| *d).sum::<i128>(),
        i128::from(held),
        "closing returns exactly the escrow"
    );
    assert_eq!(
        (0..DEVICES)
            .map(|i| market.balance(&device(i)))
            .sum::<u64>(),
        total
    );
}

#[test]
fn a_consumer_that_cannot_pay_and_an_unknown_sla_are_refused() {
    let mut market = Market::default();
    let (a, b) = (device(1), device(2));
    market.fund(a, 1_000);
    market.fund(b, 5);
    let id = market
        .open(Sla {
            provider: a,
            consumer: b,
            resource: Resource::Power,
            price_per_unit: 3,
            committed_per_tick: 2,
            floor_bps: 10_000,
            slash_per_breach: 10,
            collateral: 100,
        })
        .unwrap();
    assert!(matches!(
        market.tick(id, 2),
        Err(BarterError::Insufficient {
            required: 6,
            available: 5
        })
    ));
    assert_eq!(market.tick(99, 1), Err(BarterError::UnknownSla(99)));
    assert!(matches!(
        market.open(Sla {
            provider: b,
            consumer: a,
            resource: Resource::Compute,
            price_per_unit: 1,
            committed_per_tick: 1,
            floor_bps: 0,
            slash_per_breach: 0,
            collateral: 6
        }),
        Err(BarterError::Insufficient { .. })
    ));
}
