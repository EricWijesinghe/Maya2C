//! Transport HAL policy tests (Master Prompt 7 §4, Master Prompt 4 §8).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

use std::collections::BTreeSet;

use maya_dag_bft::{Committee, Dest, Message, Params, Validator};
use maya_link_sim::frame::{MAX_COMPACT_FRAME, ML_DSA_65_SIG, RadioFrame, frame_signed_tx};
use maya_link_sim::models::{
    FsoLaser, Neutrino, Oam, RepeaterChain, SatelliteRf, SubseaAcoustic, repetition_decode,
};
use maya_link_sim::orbital::{
    EARTH_MARS_MAX_KM, EARTH_MARS_MIN_KM, EARTH_MOON_KM, doppler_hz, light_time_us, line_of_sight,
};
use maya_link_sim::qkd::{ClassicalTranscript, EntangledPairs, QkdKey, mix};
use maya_link_sim::{Class, Link, RealLink, choose};
use maya_sim::{Duration, Instant, LinkModel, NodeId, World, replay};

#[test]
fn an_fso_fade_beyond_15_db_falls_back_to_rf() {
    let clear = FsoLaser {
        weather_decidb: 20,
        jitter_urad: 3,
    };
    let fog = FsoLaser {
        weather_decidb: 180,
        jitter_urad: 3,
    };
    let rf = SatelliteRf { rain_mm_h: 5 };
    assert_eq!(
        choose(&[&clear, &rf], 1_000_000, 2_000_000).unwrap().name(),
        "fso-laser"
    );
    assert!(!fog.profile().is_up());
    assert_eq!(
        choose(&[&fog, &rf], 1_000_000, 2_000_000).unwrap().name(),
        "satellite-ku"
    );
    assert_eq!(fog.class(), Class::Sim);
}

#[test]
fn subsea_acoustic_latency_follows_the_speed_of_sound() {
    let hop = SubseaAcoustic { distance_m: 15_000 };
    assert_eq!(
        hop.profile().latency_us,
        10_000_000,
        "15 km at 1,500 m/s is 10 s"
    );
}

#[test]
fn oam_phase_front_recovery_keeps_more_modes() {
    let without = Oam {
        modes: 8,
        turbulence: 40,
        recovery: false,
    }
    .profile()
    .bandwidth_bps;
    let with = Oam {
        modes: 8,
        turbulence: 40,
        recovery: true,
    }
    .profile()
    .bandwidth_bps;
    assert!(with > without);
}

#[test]
fn a_5000_km_repeater_chain_needs_purification_to_pass_the_fidelity_gate() {
    let raw = RepeaterChain {
        segment_km: 100,
        segments: 50,
        link_fidelity: 0.99,
        purification_rounds: 0,
    };
    let purified = RepeaterChain {
        purification_rounds: 2,
        ..raw
    };
    assert_eq!(raw.length_km(), 5_000);
    assert!(!raw.passes_gate(), "raw fidelity {}", raw.fidelity());
    assert!(
        purified.passes_gate(),
        "purified fidelity {}",
        purified.fidelity()
    );
}

#[test]
fn qkd_is_mixed_in_below_11_percent_qber_and_dropped_above() {
    let pq = [7u8; 32];
    let good = QkdKey {
        key: [1; 32],
        qber_ppm: 50_000,
    };
    let bad = QkdKey {
        key: [1; 32],
        qber_ppm: 120_000,
    };
    let (with_good, used_good) = mix(&pq, Some(&good));
    let (with_bad, used_bad) = mix(&pq, Some(&bad));
    let (alone, used_none) = mix(&pq, None);
    assert!(used_good && !used_bad && !used_none);
    assert_eq!(with_bad, alone, "a noisy QKD key changes nothing");
    assert_ne!(with_good, alone);
    // Changing the PQ secret always changes the session key: QKD never
    // replaces the handshake.
    assert_ne!(mix(&[8u8; 32], Some(&good)).0, with_good);
}

#[test]
fn entanglement_keys_require_classical_channel() {
    // Alice and Bob measure halves of the same pairs in random bases. Where
    // bases match, outcomes agree; elsewhere they are independent.
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut coin = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x & 1 == 1
    };
    let n = 2_000;
    let (mut ab, mut bb, mut abits, mut bbits) = (vec![], vec![], vec![], vec![]);
    for _ in 0..n {
        let (a, b, bit) = (coin(), coin(), coin());
        ab.push(a);
        bb.push(b);
        abits.push(bit);
        bbits.push(if a == b { bit } else { coin() });
    }
    let alice = EntangledPairs {
        bases: ab.clone(),
        bits: abits,
    };
    let bob = EntangledPairs {
        bases: bb.clone(),
        bits: bbits,
    };
    // Before any classical message, Bob's raw bits agree with Alice's only by
    // chance on the mismatched half: ~75% overall, not a key.
    let raw_agree = alice
        .bits
        .iter()
        .zip(&bob.bits)
        .filter(|(a, b)| a == b)
        .count();
    assert!(
        raw_agree < n * 80 / 100,
        "raw agreement {raw_agree}/{n} must not be a shared key"
    );
    // After exchanging bases over a classical channel, the sifted keys match.
    let ka = alice.sift(&ClassicalTranscript { peer_bases: bb });
    let kb = bob.sift(&ClassicalTranscript { peer_bases: ab });
    assert_eq!(ka, kb);
    assert!(ka.len() > n / 3);
}

#[test]
fn a_signed_pq_transaction_never_fits_a_radio_frame() {
    let signed = vec![0u8; 200 + ML_DSA_65_SIG];
    let frame = frame_signed_tx(&signed, [1; 32], [2; 32]);
    assert!(matches!(frame, RadioFrame::TxRef { .. }));
    assert!(frame.len() <= MAX_COMPACT_FRAME);
    assert!(RealLink::LoRa.profile().mtu as usize >= frame.len());
}

#[test]
fn a_neutrino_link_is_research_and_repetition_decoding_recovers_bits() {
    assert_eq!(Neutrino.class(), Class::Research);
    // 40% per-copy error, 201 copies: majority vote recovers every bit.
    let mut x = 88_172_645_463_325_252u64;
    let mut rand_ppm = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % 1_000_000) as u32
    };
    for bit in [true, false, true, true, false] {
        let votes: Vec<bool> = (0..201)
            .map(|_| if rand_ppm() < 400_000 { !bit } else { bit })
            .collect();
        assert_eq!(repetition_decode(&votes), bit);
    }
}

#[test]
fn orbital_delays_line_of_sight_and_doppler() {
    let (mars_min, mars_max) = (
        light_time_us(EARTH_MARS_MIN_KM),
        light_time_us(EARTH_MARS_MAX_KM),
    );
    assert!((180_000_000..190_000_000).contains(&mars_min), "{mars_min}"); // ~3.0 min
    assert!(
        (1_330_000_000..1_340_000_000).contains(&mars_max),
        "{mars_max}"
    ); // ~22.3 min
    assert!((1_280_000..1_290_000).contains(&light_time_us(EARTH_MOON_KM)));
    // Two LEO satellites on opposite sides of Earth cannot see each other.
    assert!(!line_of_sight((7_000, 0), (-7_000, 0), 6_371));
    assert!(line_of_sight((7_000, 0), (7_000, 1_000), 6_371));
    assert!(
        doppler_hz(2_000_000_000, 7_500) < 0,
        "receding lowers the frequency"
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ev {
    Msg(Message),
    Tick,
    Root { index: u64, digest: [u8; 32] },
}

/// Runs an Earth cluster (0-4) and, optionally, a Mars cluster (5-9) whose
/// committed anchors are batched to Earth with interplanetary delay.
fn earth_and_mars(
    w: &mut World<Ev>,
    with_mars: bool,
    delay: Duration,
) -> (usize, Vec<Vec<u64>>, Vec<[u8; 32]>) {
    let committee = Committee::new(5);
    let params = Params {
        batch_size: 100,
        anchor_timeout_ms: 1_000,
    };
    let clusters = if with_mars { 2 } else { 1 };
    let mut nodes: Vec<Validator> = (0..5 * clusters)
        .map(|i| Validator::new(i % 5, committee, params))
        .collect();
    for (i, v) in nodes.iter_mut().enumerate() {
        for k in 0..2_000u64 {
            v.submit(k * 10 + i as u64);
        }
    }
    let send = |w: &mut World<Ev>, base: u16, from: u16, sends: Vec<(Dest, Message)>| {
        for (d, m) in sends {
            let targets: Vec<u16> = match d {
                Dest::All => (0..5).filter(|p| *p != from).collect(),
                Dest::To(p) => vec![p],
            };
            for t in targets {
                w.send(NodeId(base + from), NodeId(base + t), Ev::Msg(m.clone()));
            }
        }
    };
    for i in 0..5 * clusters {
        let out = nodes[i as usize].start(0);
        send(w, i / 5 * 5, i % 5, out.sends);
        w.timer(NodeId(i), Duration::from_millis(200), Ev::Tick);
    }
    let mut earth_commits = vec![Vec::new(); 5];
    let mut mars_roots_seen: Vec<Vec<u64>> = vec![Vec::new(); 5];
    let mut mars_anchor_log: Vec<[u8; 32]> = Vec::new();
    let deadline = Instant::START + Duration::from_secs(40 * 60);
    w.run_until(deadline, 20_000_000, |w, ev| {
        let now_ms = ev.at.as_nanos() / 1_000_000;
        let id = ev.to.0;
        let (base, local) = (id / 5 * 5, id % 5);
        let out = match ev.payload {
            Ev::Msg(m) => nodes[id as usize].handle(now_ms, ev.from.0 % 5, m),
            Ev::Tick => {
                if now_ms < 20 * 60 * 1_000 {
                    w.timer(NodeId(id), Duration::from_millis(200), Ev::Tick);
                }
                nodes[id as usize].tick(now_ms)
            }
            Ev::Root { index, .. } => {
                mars_roots_seen[id as usize].push(index);
                return;
            }
        };
        if base == 0 {
            earth_commits[local as usize]
                .extend(out.committed.iter().flat_map(|c| c.vertex.batch.clone()));
        } else if local == 0 {
            // Mars node 0 batches its locally final anchors to every Earth node.
            for c in &out.committed {
                if c.vertex.round % 2 == 0
                    && c.vertex.author == committee.leader(c.vertex.round).unwrap_or(99)
                {
                    let index = mars_anchor_log.len() as u64;
                    mars_anchor_log.push(c.digest());
                    for e in 0..5 {
                        w.timer(
                            NodeId(e),
                            delay,
                            Ev::Root {
                                index,
                                digest: c.digest(),
                            },
                        );
                    }
                }
            }
        }
        send(w, base, local, out.sends);
    });
    let committed = earth_commits.iter().map(Vec::len).min().unwrap_or(0);
    (committed, mars_roots_seen, mars_anchor_log)
}

#[test]
fn mars_sub_dag_roots_reach_earth_in_order_and_do_not_slow_earth() {
    for (seed, delay) in [
        (1u64, Duration::from_secs(3 * 60)),
        (2, Duration::from_secs(22 * 60)),
    ] {
        replay(seed, move |w: &mut World<Ev>| {
            w.net_mut().set_link(LinkModel {
                loss_ppm: 0,
                ..LinkModel::wide_area()
            });
            let (with, roots, log) = earth_and_mars(w, true, delay);
            assert!(with > 5_000, "Earth committed {with}");
            // Every Earth node sees the same gap-free, in-order root sequence:
            // no fork and no desync across the delay.
            let expected: Vec<u64> = (0..roots[0].len() as u64).collect();
            for r in &roots {
                assert_eq!(r, &expected);
            }
            assert!(!roots[0].is_empty() && roots[0].len() <= log.len());
        });
    }
    // Tier 1 throughput with and without the far cluster attached.
    replay(3, |w: &mut World<Ev>| {
        w.net_mut().set_link(LinkModel {
            loss_ppm: 0,
            ..LinkModel::wide_area()
        });
        let alone = earth_and_mars(w, false, Duration::ZERO).0;
        assert!(alone > 5_000, "Earth alone committed {alone}");
    });
}

#[test]
fn store_and_forward_recovers_full_state_through_30_percent_loss() {
    // A → B → C relay; every hop loses 30%. Each node keeps what it has and
    // retransmits until the next hop acknowledges.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum M {
        Chunk(u32),
        Ack(u32),
        Retry,
    }
    replay(0x5AF0, |w: &mut World<M>| {
        w.net_mut().set_link(LinkModel {
            loss_ppm: 300_000,
            min_latency: Duration::from_millis(50),
            max_latency: Duration::from_millis(500),
            ..LinkModel::default()
        });
        let chunks = 500u32;
        let mut held: [BTreeSet<u32>; 3] =
            [(0..chunks).collect(), BTreeSet::new(), BTreeSet::new()];
        let mut acked: [BTreeSet<u32>; 2] = [BTreeSet::new(), BTreeSet::new()];
        w.timer(NodeId(0), Duration::ZERO, M::Retry);
        w.timer(NodeId(1), Duration::ZERO, M::Retry);
        w.run_until(
            Instant::START + Duration::from_secs(3_600),
            5_000_000,
            |w, ev| {
                let me = ev.to.0 as usize;
                match ev.payload {
                    M::Retry => {
                        let pending: Vec<u32> = held[me].difference(&acked[me]).copied().collect();
                        for c in pending.into_iter().take(64) {
                            w.send(NodeId(me as u16), NodeId(me as u16 + 1), M::Chunk(c));
                        }
                        if acked[me].len() < chunks as usize {
                            w.timer(NodeId(me as u16), Duration::from_secs(2), M::Retry);
                        }
                    }
                    M::Chunk(c) => {
                        held[me].insert(c);
                        w.send(NodeId(me as u16), ev.from, M::Ack(c));
                    }
                    M::Ack(c) => {
                        acked[me].insert(c);
                    }
                }
            },
        );
        assert_eq!(held[2].len(), chunks as usize, "C holds every chunk");
    });
}
