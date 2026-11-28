//! A joining node syncs from peers, some of them lying (Master Prompt 14 §2).

use maya_state_sync::sim::{Setup, simulate};
use maya_state_sync::{Downloader, Event, Manifest, SyntheticState, chunk_hash};

fn expected_sum(state: SyntheticState) -> u128 {
    (0..state.chunks())
        .map(|i| SyntheticState::balance_sum(&state.chunk(i)))
        .sum()
}

#[test]
fn a_node_syncs_a_million_accounts_and_bans_the_peers_that_lie() {
    let state = SyntheticState {
        accounts: 1_000_000,
        per_chunk: 10_000,
    };
    let setup = Setup {
        peers: 12,
        malicious: vec![3, 7],
        per_peer: 4,
        uplink_bps: 100_000_000,
        seed: 1,
    };
    let r = simulate(state, &setup);
    assert_eq!(
        r.banned,
        vec![3, 7],
        "both liars caught, no honest peer banned"
    );
    assert_eq!(
        r.balance_sum,
        expected_sum(state),
        "every record imported exactly once"
    );
    assert!(
        r.wasted > 0 && r.wasted <= 8 * 480_000,
        "a lie costs at most what was in flight from the liar"
    );
    println!(
        "1M accounts: {:.1} virtual s, {} MB, {} KB wasted",
        r.virtual_secs,
        r.bytes / 1_000_000,
        r.wasted / 1_000
    );
}

#[test]
fn every_chunk_proves_and_a_substituted_chunk_does_not() {
    let state = SyntheticState {
        accounts: 55_555,
        per_chunk: 1_000,
    };
    let m = Manifest::new(
        (0..state.chunks())
            .map(|i| chunk_hash(&state.chunk(i)))
            .collect(),
    );
    for i in 0..state.chunks() {
        let p = m.prove(i);
        assert!(p.verify(&m.root(), m.chunks(), &state.chunk(i)));
        assert!(!p.verify(
            &m.root(),
            m.chunks(),
            &state.chunk((i + 1) % state.chunks())
        ));
    }
}

#[test]
fn if_every_peer_lies_the_sync_stalls_rather_than_accepting_anything() {
    let state = SyntheticState {
        accounts: 50_000,
        per_chunk: 10_000,
    };
    let setup = Setup {
        peers: 3,
        malicious: vec![1, 2, 3],
        per_peer: 2,
        uplink_bps: 100_000_000,
        seed: 2,
    };
    let r = simulate(state, &setup);
    assert_eq!(r.banned, vec![1, 2, 3]);
    assert!(
        r.balance_sum < expected_sum(state),
        "incomplete, and nothing false imported"
    );
}

// ---------------------------------------------------------------------------
// red team: proofs in the wrong shape, and peers that misbehave
// ---------------------------------------------------------------------------

/// `n` distinct chunks and the manifest over them.
fn snapshot(n: u32) -> (Vec<Vec<u8>>, Manifest) {
    let chunks: Vec<Vec<u8>> = (0..n).map(|i| i.to_le_bytes().repeat(25)).collect();
    let m = Manifest::new(chunks.iter().map(|c| chunk_hash(c)).collect());
    (chunks, m)
}

#[test]
fn a_proof_with_a_sibling_added_dropped_or_moved_is_refused() {
    // 7 chunks: the last one is promoted at the first level.
    let (chunks, m) = snapshot(7);
    for i in 0..7 {
        let p = m.prove(i);
        let chunk = &chunks[i as usize];
        assert!(p.verify(&m.root(), 7, chunk), "chunk {i}");
        let mut extra = p.clone();
        extra.siblings.push([3; 32]);
        assert!(
            !extra.verify(&m.root(), 7, chunk),
            "chunk {i}: extra sibling"
        );
        if !p.siblings.is_empty() {
            let mut short = p.clone();
            short.siblings.pop();
            assert!(!short.verify(&m.root(), 7, chunk), "chunk {i}: short");
        }
        for j in (0..7).filter(|&j| j != i) {
            let moved = maya_state_sync::ChunkProof {
                index: j,
                ..p.clone()
            };
            assert!(!moved.verify(&m.root(), 7, chunk), "chunk {i} at {j}");
        }
    }
    let past_end = maya_state_sync::ChunkProof {
        index: 7,
        siblings: m.prove(6).siblings,
    };
    assert!(!past_end.verify(&m.root(), 7, &chunks[6]));
}

#[test]
fn two_child_hashes_cannot_pass_as_one_chunk() {
    // The second-preimage attack on an untagged tree: present an interior
    // node's children as a chunk of a one-chunk snapshot with the same root.
    let (chunks, m) = snapshot(2);
    let forged: Vec<u8> = [chunk_hash(&chunks[0]), chunk_hash(&chunks[1])].concat();
    let p = maya_state_sync::ChunkProof {
        index: 0,
        siblings: vec![],
    };
    assert!(!p.verify(&m.root(), 1, &forged));
}

#[test]
fn a_one_chunk_snapshot_proves_and_an_empty_one_proves_nothing() {
    let (chunks, m) = snapshot(1);
    assert_eq!(m.root(), chunk_hash(&chunks[0]));
    assert!(m.prove(0).verify(&m.root(), 1, &chunks[0]));
    let empty = Manifest::new(vec![]);
    assert_eq!(empty.chunks(), 0);
    let p = maya_state_sync::ChunkProof {
        index: 0,
        siblings: vec![],
    };
    assert!(!p.verify(&empty.root(), 0, &[]));
    let d = Downloader::new(empty.root(), 0, [1], 4);
    assert!(d.is_complete());
}

#[test]
fn requests_go_lowest_chunk_first_and_never_past_the_per_peer_limit() {
    let (_, m) = snapshot(20);
    let mut d = Downloader::new(m.root(), 20, [1, 2, 3], 2);
    let first = d.poll();
    assert_eq!(first.len(), 6, "three peers, two each");
    let mut asked: Vec<u32> = first.iter().map(|r| r.chunk).collect();
    asked.sort_unstable();
    assert_eq!(asked, (0..6).collect::<Vec<_>>());
    for peer in [1, 2, 3] {
        assert_eq!(first.iter().filter(|r| r.peer == peer).count(), 2);
    }
    assert!(d.poll().is_empty(), "everyone is at the limit");
}

#[test]
fn a_valid_chunk_under_the_wrong_index_bans_the_peer() {
    let (chunks, m) = snapshot(4);
    let mut d = Downloader::new(m.root(), 4, [1, 2], 1);
    let reqs = d.poll();
    let liar = reqs[0];
    // Asked for chunk c, the peer sends a genuine other chunk with its genuine proof.
    let other = (liar.chunk + 1) % 4;
    let events = d.receive(
        liar.peer,
        liar.chunk,
        &chunks[other as usize],
        &m.prove(other),
    );
    assert_eq!(events, vec![Event::Banned(liar.peer)]);
    assert!(d.banned().contains(&liar.peer));
    assert!(d.wasted > 0);
}

#[test]
fn unsolicited_duplicate_and_post_ban_responses_change_nothing() {
    let (chunks, m) = snapshot(4);
    let mut d = Downloader::new(m.root(), 4, [1, 2], 1);
    let reqs = d.poll();
    let (a, b) = (reqs[0], reqs[1]);

    // A chunk nobody asked this peer for, even a genuine one: ignored, not counted.
    let unasked = 3;
    assert!(
        d.receive(a.peer, unasked, &chunks[3], &m.prove(unasked))
            .is_empty()
    );
    assert_eq!((d.bytes, d.wasted), (0, 0));
    assert!(d.banned().is_empty());

    // The real answer verifies once; the same answer again is ignored.
    let good = d.receive(
        a.peer,
        a.chunk,
        &chunks[a.chunk as usize],
        &m.prove(a.chunk),
    );
    assert_eq!(good, vec![Event::Verified(a.chunk)]);
    let bytes = d.bytes;
    assert!(
        d.receive(
            a.peer,
            a.chunk,
            &chunks[a.chunk as usize],
            &m.prove(a.chunk)
        )
        .is_empty()
    );
    assert_eq!(d.bytes, bytes);

    // Peer b lies, is banned, and a later genuine answer from it is ignored.
    assert_eq!(
        d.receive(b.peer, b.chunk, b"garbage", &m.prove(b.chunk)),
        vec![Event::Banned(b.peer)]
    );
    assert!(
        d.receive(
            b.peer,
            b.chunk,
            &chunks[b.chunk as usize],
            &m.prove(b.chunk)
        )
        .is_empty()
    );
    assert!(
        d.poll().iter().all(|r| r.peer != b.peer),
        "a banned peer is never asked again"
    );
}

#[test]
fn a_liars_owed_chunks_are_fetched_from_honest_peers_and_each_imported_once() {
    let n = 9;
    let (chunks, m) = snapshot(n);
    let mut d = Downloader::new(m.root(), n, [1, 2], 3);
    let liar = 2;
    let mut verified = Vec::new();
    let mut complete = 0;
    for _ in 0..100 {
        let reqs = d.poll();
        if reqs.is_empty() {
            break;
        }
        for r in reqs {
            let bytes: &[u8] = if r.peer == liar {
                b"not the chunk"
            } else {
                &chunks[r.chunk as usize]
            };
            for e in d.receive(r.peer, r.chunk, bytes, &m.prove(r.chunk)) {
                match e {
                    Event::Verified(c) => verified.push(c),
                    Event::Complete => complete += 1,
                    Event::Banned(p) => assert_eq!(p, liar),
                }
            }
        }
    }
    assert!(d.is_complete());
    assert_eq!(complete, 1, "completion is reported once");
    verified.sort_unstable();
    assert_eq!(
        verified,
        (0..n).collect::<Vec<_>>(),
        "every chunk exactly once"
    );
    assert_eq!(d.banned().iter().copied().collect::<Vec<_>>(), vec![liar]);
}
