//! A joining node syncs from peers, some of them lying (Master Prompt 14 §2).

use maya_state_sync::sim::{Setup, simulate};
use maya_state_sync::{Manifest, SyntheticState, chunk_hash};

fn expected_sum(state: SyntheticState) -> u128 {
    (0..state.chunks()).map(|i| SyntheticState::balance_sum(&state.chunk(i))).sum()
}

#[test]
fn a_node_syncs_a_million_accounts_and_bans_the_peers_that_lie() {
    let state = SyntheticState { accounts: 1_000_000, per_chunk: 10_000 };
    let setup = Setup { peers: 12, malicious: vec![3, 7], per_peer: 4, uplink_bps: 100_000_000, seed: 1 };
    let r = simulate(state, &setup);
    assert_eq!(r.banned, vec![3, 7], "both liars caught, no honest peer banned");
    assert_eq!(r.balance_sum, expected_sum(state), "every record imported exactly once");
    assert!(r.wasted > 0 && r.wasted <= 8 * 480_000, "a lie costs at most what was in flight from the liar");
    println!("1M accounts: {:.1} virtual s, {} MB, {} KB wasted", r.virtual_secs, r.bytes / 1_000_000, r.wasted / 1_000);
}

#[test]
fn every_chunk_proves_and_a_substituted_chunk_does_not() {
    let state = SyntheticState { accounts: 55_555, per_chunk: 1_000 };
    let m = Manifest::new((0..state.chunks()).map(|i| chunk_hash(&state.chunk(i))).collect());
    for i in 0..state.chunks() {
        let p = m.prove(i);
        assert!(p.verify(&m.root(), m.chunks(), &state.chunk(i)));
        assert!(!p.verify(&m.root(), m.chunks(), &state.chunk((i + 1) % state.chunks())));
    }
}

#[test]
fn if_every_peer_lies_the_sync_stalls_rather_than_accepting_anything() {
    let state = SyntheticState { accounts: 50_000, per_chunk: 10_000 };
    let setup = Setup { peers: 3, malicious: vec![1, 2, 3], per_peer: 2, uplink_bps: 100_000_000, seed: 2 };
    let r = simulate(state, &setup);
    assert_eq!(r.banned, vec![1, 2, 3]);
    assert!(r.balance_sum < expected_sum(state), "incomplete, and nothing false imported");
}
