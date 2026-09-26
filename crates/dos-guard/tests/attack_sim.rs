//! [SIM] Adversarial scenarios from Master Prompt 16 §4, run with and without
//! the defences.
//!
//! A deterministic model, not a network: 100 ms ticks, a node with 4 cores of
//! verification time per tick, and a hybrid verification costing 1,000 µs (the
//! measured 0.99 ms, `reports/12-baseline.md`). Honest traffic must stay
//! inside the finality budget: admitted within 1 s (half the 2 s p50 finality
//! SLO), measured at p99.

#![allow(
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use std::collections::VecDeque;

use maya_dos_guard::Addr;
use maya_dos_guard::budget::{Budgets, Rate};
use maya_dos_guard::cookie::CookieJar;
use maya_dos_guard::diversity::{Limits, Table};
use maya_dos_guard::ledger::{Ledger, Thresholds, Verdict};

const TICK_MS: u64 = 100;
const CPU_PER_TICK_US: u64 = 4 * 100_000;
const VERIFY_US: u64 = 1_000;
const SLO_MS: u64 = 1_000;

#[derive(Clone, Copy)]
struct Msg {
    peer: u32,
    honest: bool,
    arrived_ms: u64,
}

/// Invalid-signature spam at 10x the honest rate. Returns (honest p99 ms, honest admitted, spam verified).
fn spam(guarded: bool) -> (u64, usize, u64) {
    let (honest_per_tick, attackers, spam_per_attacker_per_tick) = (80u32, 10u32, 80u32); // 800 tx/s honest, 8,000/s spam
    let mut ledger = Ledger::new(Thresholds {
        throttle: 20_000,
        disconnect: 50_000,
    });
    let mut queue: VecDeque<Msg> = VecDeque::new();
    let mut latencies = Vec::new();
    let mut spam_verified = 0;
    let mut disconnected = [false; 10];
    for tick in 0..600u64 {
        let now = tick * TICK_MS;
        for i in 0..honest_per_tick {
            queue.push_back(Msg {
                peer: 1_000 + i % 20,
                honest: true,
                arrived_ms: now,
            });
        }
        for a in 0..attackers {
            if !(guarded && disconnected[a as usize]) {
                for _ in 0..spam_per_attacker_per_tick {
                    queue.push_back(Msg {
                        peer: a,
                        honest: false,
                        arrived_ms: now,
                    });
                }
            }
        }
        let mut cpu = CPU_PER_TICK_US;
        while cpu >= VERIFY_US {
            let Some(m) = queue.pop_front() else { break };
            if guarded && m.peer < attackers && disconnected[m.peer as usize] {
                continue; // dropped at the connection, no verification spent
            }
            cpu -= VERIFY_US;
            if m.honest {
                latencies.push(now + TICK_MS - m.arrived_ms);
                ledger.credit(m.peer, VERIFY_US, now);
            } else {
                spam_verified += 1;
                ledger.charge(m.peer, VERIFY_US, now);
                if guarded && ledger.verdict(m.peer, now) == Verdict::Disconnect {
                    disconnected[m.peer as usize] = true;
                }
            }
        }
    }
    latencies.sort_unstable();
    let p99 = latencies
        .get(latencies.len() * 99 / 100)
        .copied()
        .unwrap_or(u64::MAX);
    (p99, latencies.len(), spam_verified)
}

#[test]
fn invalid_signature_spam_at_ten_times_load() {
    let (open_p99, open_n, open_spam) = spam(false);
    let (p99, n, spam_verified) = spam(true);
    println!(
        "[SIM] spam 10x: unguarded honest p99 {open_p99} ms ({open_n} admitted, {open_spam} spam verified); guarded p99 {p99} ms ({n} admitted, {spam_verified} spam verified)"
    );
    assert!(
        open_p99 > SLO_MS,
        "the scenario must actually overload an unguarded node"
    );
    assert!(
        p99 <= SLO_MS,
        "honest traffic must stay inside the SLO under the guard"
    );
    assert!(spam_verified < open_spam / 100);
}

#[test]
fn handshake_flood_does_no_kem_work_for_spoofed_or_over_budget_sources() {
    let jar = CookieJar::new([7; 32]);
    let mut budgets = Budgets::new(
        Rate {
            per_sec: 2,
            burst: 4,
        },
        Rate {
            per_sec: 5,
            burst: 10,
        },
    );
    let (mut kem_ops, mut honest_ok, mut honest_tried) = (0u64, 0u64, 0u64);
    for tick in 0..600u64 {
        let now = tick * TICK_MS;
        // 5,000 hellos/s from spoofed addresses: they never receive their cookie.
        for i in 0..500u32 {
            let spoofed = Addr::V4([10, (i >> 8) as u8, i as u8, (tick % 250) as u8]);
            let guess = [0u8; 16];
            if jar.check(spoofed, &guess, now) {
                kem_ops += 1;
            }
        }
        // 50 real attacker hosts in one /24, each retrying every tick with a valid cookie.
        for h in 0..50u8 {
            let a = Addr::V4([66, 6, 6, h]);
            if jar.check(a, &jar.issue(a, now), now) && budgets.admit(a, now) {
                kem_ops += 1;
            }
        }
        // 20 honest peers in 20 subnets, one handshake per second each.
        if tick % 10 == 0 {
            for p in 0..20u8 {
                let a = Addr::V4([100 + p, 1, 1, 1]);
                honest_tried += 1;
                if jar.check(a, &jar.issue(a, now), now) && budgets.admit(a, now) {
                    kem_ops += 1;
                    honest_ok += 1;
                }
            }
        }
    }
    println!(
        "[SIM] handshake flood (60 s, 5,000/s spoofed + 50 hosts in one /24): {kem_ops} KEM operations, honest {honest_ok}/{honest_tried}"
    );
    assert_eq!(honest_ok, honest_tried, "every honest handshake completes");
    // The attacker /24 is held to its subnet budget: 10 burst + 5/s.
    assert!(kem_ops <= honest_tried + 10 + 5 * 60 + 5);
}

#[test]
fn slow_loris_peers_are_disconnected_and_honest_peers_are_not() {
    let mut ledger = Ledger::new(Thresholds {
        throttle: 2_000,
        disconnect: 8_000,
    });
    let mut cut_at = None;
    for tick in 0..600u64 {
        let now = tick * TICK_MS;
        // A slow-loris peer holds 64 KiB of half-sent frames, contributing nothing:
        // charged 1 unit per KiB-tick held.
        ledger.charge(1u32, 64, now);
        // An honest peer holds 4 KiB briefly and delivers valid messages.
        ledger.charge(2u32, 4, now);
        ledger.credit(2u32, 8, now);
        if cut_at.is_none() && ledger.verdict(1, now) == Verdict::Disconnect {
            cut_at = Some(now);
        }
        assert_ne!(
            ledger.verdict(2, now),
            Verdict::Disconnect,
            "honest peer cut at {now} ms"
        );
    }
    println!(
        "[SIM] slow-loris: disconnected after {} ms; honest peer never throttled",
        cut_at.unwrap()
    );
    assert!(cut_at.unwrap() <= 30_000);
}

#[test]
fn an_attacker_with_70_percent_of_connection_attempts_gets_a_minority_of_connections() {
    let mut t = Table::new(Limits {
        inbound: 40,
        per_subnet: 2,
        outbound: 10,
    });
    let mut attacker_in = 0;
    // 700 attempts from 3 attacker subnets interleaved with 300 from 150 honest subnets.
    for i in 0..1_000u32 {
        let attacker = i % 10 < 7;
        let a = if attacker {
            Addr::V4([66, 6, (i % 3) as u8, (i % 250) as u8])
        } else {
            Addr::V4([(i % 150) as u8 + 1, 2, 3, 4])
        };
        if t.accept_inbound(a) && attacker {
            attacker_in += 1;
        }
    }
    // Outbound: the node dials from an address book the attacker has also stuffed 70%.
    let mut attacker_out = 0;
    for i in 0..100u32 {
        let attacker = i % 10 < 7;
        let a = if attacker {
            Addr::V4([66, 6, (i % 3) as u8, (i % 250) as u8])
        } else {
            Addr::V4([200, (i % 50) as u8, 0, 1])
        };
        if t.dial(a) && attacker {
            attacker_out += 1;
        }
    }
    let total = 40 + 10;
    let attacker_total = attacker_in + attacker_out;
    println!(
        "[SIM] eclipse at 70% of attempts: attacker holds {attacker_total}/{total} connections ({attacker_in} inbound, {attacker_out} outbound)"
    );
    assert!(
        attacker_total * 2 < total,
        "the victim keeps an honest majority"
    );
}
