//! The relay end to end, without a kernel.
//!
//! A block is sealed into datagrams and handed to a receiver reordered,
//! duplicated, partly lost, tampered with, or sent under keys the receiver does
//! not hold. The properties:
//!
//! - every ordering, with any duplicates, delivers the body exactly once and
//!   byte-identical;
//! - nothing is delivered while a chunk is missing;
//! - a datagram that fails authentication blames nobody, because its key id is
//!   public and its source address is whatever its sender wrote;
//! - what an authenticated peer could not have sent honestly is attributed to
//!   that peer, and to no one else;
//! - no sequence of bytes panics the receiver or makes it deliver anything.

use std::time::Instant;

use maya_ebpf_net::common::header::{CHUNK_LEN, HEADER_LEN, MAX_DATAGRAM_LEN};
use maya_ebpf_net::{
    BlockSealer, Dropped, KeyBook, Limits, Misbehaviour, RelayEvent, RelayKey, RelayReceiver,
    Role, SharedKeyBook, derive_keys,
};
use proptest::prelude::*;

const ALICE: u8 = 1;
const BOB: u8 = 2;
const BLOCK_ID: [u8; 32] = [0xB1; 32];

/// `peer`'s outbound key, with the matching inbound key registered in `book`.
fn connect(book: &SharedKeyBook<u8>, peer: u8) -> RelayKey {
    let name = [peer];
    let theirs = derive_keys(&[peer; 32], &[0xEE; 32], &name, b"node", Role::Requester);
    let ours = derive_keys(&[peer; 32], &[0xEE; 32], &name, b"node", Role::Responder);
    book.write().unwrap().insert(peer, ours.inbound);
    theirs.outbound
}

fn receiver(book: &SharedKeyBook<u8>) -> RelayReceiver<u8> {
    RelayReceiver::new(book.clone(), Limits::DEFAULT)
}

fn datagrams(key: &RelayKey, body: &[u8]) -> Vec<Vec<u8>> {
    let sealer = BlockSealer::new(key, BLOCK_ID, body).unwrap();
    (0..sealer.chunk_count())
        .map(|index| {
            let mut out = Vec::new();
            sealer.seal(index, &mut out).unwrap();
            out
        })
        .collect()
}

/// Fisher-Yates over a xorshift stream: deterministic for a given seed.
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed | 1;
    for i in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(i, (state % (i as u64 + 1)) as usize);
    }
}

fn feed(receiver: &mut RelayReceiver<u8>, datagrams: &[Vec<u8>]) -> Vec<RelayEvent<u8>> {
    let now = Instant::now();
    datagrams
        .iter()
        .filter_map(|datagram| receiver.ingest(datagram, now).ok().flatten())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_order_with_duplicates_delivers_the_body_exactly_once(
        body in proptest::collection::vec(any::<u8>(), 1..CHUNK_LEN * 6),
        seed in any::<u64>(),
        duplicates in 0usize..8,
    ) {
        let book = KeyBook::shared();
        let key = connect(&book, ALICE);
        let mut sent = datagrams(&key, &body);
        prop_assert!(sent.iter().all(|d| d.len() <= MAX_DATAGRAM_LEN));
        let extra: Vec<Vec<u8>> = sent.iter().cycle().take(duplicates).cloned().collect();
        sent.extend(extra);
        shuffle(&mut sent, seed);

        let events = feed(&mut receiver(&book), &sent);
        prop_assert_eq!(events, vec![RelayEvent::Block { peer: ALICE, block_id: BLOCK_ID, body }]);
    }

    #[test]
    fn arbitrary_bytes_never_panic_and_never_deliver(
        bytes in proptest::collection::vec(any::<u8>(), 0..MAX_DATAGRAM_LEN * 2),
    ) {
        let book = KeyBook::shared();
        let _key = connect(&book, ALICE);
        let mut receiver = receiver(&book);
        for datagram in [&bytes[..], &bytes[..bytes.len() / 2]] {
            prop_assert!(matches!(receiver.ingest(datagram, Instant::now()), Err(_) | Ok(None)));
        }
    }
}

#[test]
fn a_lost_chunk_withholds_the_block_until_it_is_resent() {
    let book = KeyBook::shared();
    let key = connect(&book, ALICE);
    let body: Vec<u8> = (0..CHUNK_LEN * 4).map(|i| (i % 251) as u8).collect();
    let sent = datagrams(&key, &body);
    let mut receiver = receiver(&book);

    let (lost, rest) = sent.split_first().unwrap();
    assert!(feed(&mut receiver, rest).is_empty());
    let events = feed(&mut receiver, std::slice::from_ref(lost));
    assert_eq!(
        events,
        [RelayEvent::Block {
            peer: ALICE,
            block_id: BLOCK_ID,
            body
        }]
    );
}

#[test]
fn a_megabyte_block_reassembles() {
    let book = KeyBook::shared();
    let key = connect(&book, ALICE);
    let body: Vec<u8> = (0..1 << 20).map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    let events = feed(&mut receiver(&book), &datagrams(&key, &body));
    assert!(matches!(&events[..], [RelayEvent::Block { body: got, .. }] if *got == body));
}

#[test]
fn a_forged_datagram_under_a_real_key_id_blames_nobody() {
    let book = KeyBook::shared();
    let key = connect(&book, ALICE);
    let sent = datagrams(&key, b"one small block");
    let mut forged = sent[0].clone();
    forged[HEADER_LEN] ^= 0x80;

    let mut receiver = receiver(&book);
    assert_eq!(receiver.ingest(&forged, Instant::now()), Err(Dropped::Forged));
    assert_eq!(receiver.stats().misbehaved, 0);
    // The honest datagram behind it is unaffected.
    assert!(matches!(
        receiver.ingest(&sent[0], Instant::now()),
        Ok(Some(RelayEvent::Block { peer: ALICE, .. }))
    ));
}

#[test]
fn datagrams_under_a_key_the_receiver_does_not_hold_are_dropped() {
    let book = KeyBook::shared();
    let _alice = connect(&book, ALICE);
    let stranger = RelayKey::from_bytes(&[0x66; 32]);
    let mut receiver = receiver(&book);
    let sent = datagrams(&stranger, b"not for you");
    assert_eq!(receiver.ingest(&sent[0], Instant::now()), Err(Dropped::UnknownKey));
}

#[test]
fn an_authenticated_conflicting_chunk_is_attributed_to_its_sender_alone() {
    let book = KeyBook::shared();
    let alice = connect(&book, ALICE);
    let bob = connect(&book, BOB);
    let honest = vec![0u8; CHUNK_LEN * 2];
    let mut lie = honest.clone();
    lie[0] = 1;

    let mut receiver = receiver(&book);
    let now = Instant::now();
    receiver.ingest(&datagrams(&bob, &honest)[0], now).unwrap();
    receiver.ingest(&datagrams(&alice, &honest)[0], now).unwrap();
    assert_eq!(
        receiver.ingest(&datagrams(&alice, &lie)[0], now),
        Ok(Some(RelayEvent::Misbehaved {
            peer: ALICE,
            what: Misbehaviour::ConflictingChunk
        }))
    );
    // Bob's reassembly of the same block id is his own, and completes.
    assert!(matches!(
        receiver.ingest(&datagrams(&bob, &honest)[1], now),
        Ok(Some(RelayEvent::Block { peer: BOB, .. }))
    ));
}

#[test]
fn a_garbage_body_from_one_peer_does_not_suppress_anothers_relay() {
    let book = KeyBook::shared();
    let alice = connect(&book, ALICE);
    let bob = connect(&book, BOB);
    let mut receiver = receiver(&book);
    let garbage = feed(&mut receiver, &datagrams(&alice, b"garbage under a real id"));
    let honest = feed(&mut receiver, &datagrams(&bob, b"the real block body"));
    assert!(matches!(&garbage[..], [RelayEvent::Block { peer: ALICE, .. }]));
    assert!(matches!(&honest[..], [RelayEvent::Block { peer: BOB, .. }]));
}

#[test]
fn receivers_sharing_a_reassembler_share_its_memory_bound_and_complete_split_blocks() {
    use std::sync::{Arc, Mutex};

    use maya_ebpf_net::{Reassembler, Refused};

    let book = KeyBook::shared();
    let key = connect(&book, ALICE);
    let body = vec![0x77u8; CHUNK_LEN * 2];
    let sent = datagrams(&key, &body);
    let limits = Limits {
        pending_bytes: body.len(),
        ..Limits::DEFAULT
    };
    let shared = Arc::new(Mutex::new(Reassembler::new(limits)));
    let mut queue_0 = RelayReceiver::with_reassembler(book.clone(), Arc::clone(&shared));
    let mut queue_1 = RelayReceiver::with_reassembler(book.clone(), Arc::clone(&shared));
    let now = Instant::now();

    // A second block started on the other queue finds the memory already used:
    // the bound is the node's, not each queue's.
    assert_eq!(queue_0.ingest(&sent[0], now), Ok(None));
    let other = BlockSealer::new(&key, [0xB2; 32], &body).unwrap();
    let mut second = Vec::new();
    other.seal(0, &mut second).unwrap();
    assert_eq!(
        queue_1.ingest(&second, now),
        Err(Dropped::Refused(Refused::MemoryFull))
    );

    // And a block whose chunks land on different queues still completes.
    assert!(matches!(
        queue_1.ingest(&sent[1], now),
        Ok(Some(RelayEvent::Block { peer: ALICE, .. }))
    ));
}

#[test]
fn forgetting_a_peer_stops_its_datagrams() {
    let book = KeyBook::shared();
    let key = connect(&book, ALICE);
    book.write().unwrap().remove_peer(ALICE);
    assert!(book.read().unwrap().is_empty());
    let mut receiver = receiver(&book);
    assert_eq!(
        receiver.ingest(&datagrams(&key, b"too late")[0], Instant::now()),
        Err(Dropped::UnknownKey)
    );
}
