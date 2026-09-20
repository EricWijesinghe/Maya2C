//! The wire format: round trips, and the malformed frames a socket will
//! eventually hand it.
//!
//! Every case here is reachable from the network. A custody protocol's decoder
//! is the first code an attacker who has authenticated reaches, and "the peer
//! held a valid certificate" is not an argument about what its next four bytes
//! say.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_custody_mpc::dkg::{Custodian, Roster, VaultPolicy};
use maya_custody_mpc::error::CustodyError;
use maya_custody_mpc::session::{SigningSession, VaultDescriptor, respond};
use maya_custody_mpc::transport::{Frame, MAX_FRAME_LEN};

fn small_ceremony() -> (Roster, Vec<Custodian>) {
    let policy = VaultPolicy::new(2, 3).expect("policy");
    let members: Vec<Custodian> = (1..=3)
        .map(|i| Custodian::begin(policy, i).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");
    (roster, members)
}

#[test]
fn an_announcement_round_trips() {
    let (_, members) = small_ceremony();
    let announcement = members[1].announce();

    let encoded = Frame::Announce(Box::new(announcement.clone())).encode();
    let Frame::Announce(decoded) = Frame::decode(&encoded).expect("decode") else {
        panic!("wrong variant");
    };

    assert_eq!(decoded.index, announcement.index);
    assert_eq!(decoded.encapsulation_key, announcement.encapsulation_key);
}

#[test]
fn a_dealing_round_trips_and_is_still_acceptable_afterwards() {
    // Not just byte equality: the decoded dealing is fed back into the protocol,
    // because a codec that round-trips its own output and produces commitment
    // points the verifier rejects is a codec that passes its own tests only.
    let (roster, members) = small_ceremony();
    let dealings: Vec<_> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();

    let ferried: Vec<_> = dealings
        .iter()
        .map(|d| {
            let encoded = Frame::Deal(Box::new(d.clone())).encode();
            match Frame::decode(&encoded).expect("decode") {
                Frame::Deal(decoded) => *decoded,
                _ => panic!("wrong variant"),
            }
        })
        .collect();

    for member in &members {
        member
            .accept(&roster, &ferried)
            .expect("a ferried dealing is still a dealing");
    }
}

#[test]
fn a_signing_request_and_its_response_round_trip() {
    let (roster, members) = small_ceremony();
    let dealings: Vec<_> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();
    let held: Vec<_> = members
        .iter()
        .map(|c| c.accept(&roster, &dealings).expect("accept"))
        .collect();

    let vault = VaultDescriptor::establish(&held[..2]).expect("establish");
    let mut session = SigningSession::open(vault, b"a transaction".to_vec());

    let encoded = Frame::Request(Box::new(session.request())).encode();
    let Frame::Request(request) = Frame::decode(&encoded).expect("decode") else {
        panic!("wrong variant");
    };
    assert_eq!(
        request.id(),
        session.id(),
        "the session id survives the wire"
    );

    for share in &held[..2] {
        let sealed = respond(&request, share).expect("respond");
        let encoded = Frame::Contribution(sealed).encode();
        let Frame::Contribution(decoded) = Frame::decode(&encoded).expect("decode") else {
            panic!("wrong variant");
        };
        session.accept_sealed(&decoded).expect("accept");
    }

    session.sign().expect("sign");
}

#[test]
fn an_unknown_tag_is_refused() {
    assert!(matches!(
        Frame::decode(&[0xFF, 0x00]),
        Err(CustodyError::Malformed(_))
    ));
}

#[test]
fn an_empty_frame_is_refused() {
    assert!(matches!(
        Frame::decode(&[]),
        Err(CustodyError::Malformed(_))
    ));
}

#[test]
fn a_truncated_frame_is_refused_rather_than_padded() {
    let (_, members) = small_ceremony();
    let encoded = Frame::Announce(Box::new(members[0].announce())).encode();

    for cut in 1..encoded.len() {
        assert!(
            matches!(
                Frame::decode(&encoded[..cut]),
                Err(CustodyError::Malformed(_))
            ),
            "a frame cut at {cut} bytes must not decode"
        );
    }
}

#[test]
fn trailing_bytes_after_a_frame_are_refused() {
    // A decoder that ignores trailing bytes accepts two byte strings for one
    // message. This protocol hashes its messages into a session id, so that is
    // a second name for one session.
    let (_, members) = small_ceremony();
    let mut encoded = Frame::Announce(Box::new(members[0].announce())).encode();
    encoded.push(0x00);

    assert!(matches!(
        Frame::decode(&encoded),
        Err(CustodyError::Malformed(_))
    ));
}

#[test]
fn a_length_prefix_larger_than_the_frame_is_refused() {
    // The allocation attack, in its simplest form: claim more sealed shares than
    // the frame contains. The reader must run out of bytes rather than reserve
    // for the claim.
    let (roster, members) = small_ceremony();
    let dealing = members[0].deal(&roster).expect("deal");
    let mut encoded = Frame::Deal(Box::new(dealing)).encode();

    // The sealed-share count sits after the tag, the dealer, and the
    // length-prefixed commitment vector.
    let commitment_count = u16::from_be_bytes([encoded[2], encoded[3]]) as usize;
    let offset = 4 + commitment_count * 32;
    encoded[offset] = 0xFF;
    encoded[offset + 1] = 0xFF;

    assert!(matches!(
        Frame::decode(&encoded),
        Err(CustodyError::Malformed(_))
    ));
}

#[test]
fn a_sealed_share_of_the_wrong_length_is_refused() {
    // A sealed share is an ML-KEM ciphertext plus an AEAD ciphertext over a
    // fixed-size plaintext. There is exactly one valid length, so any other is a
    // malformed message rather than a variant to accommodate.
    let (roster, members) = small_ceremony();
    let dealing = members[0].deal(&roster).expect("deal");
    let mut encoded = Frame::Deal(Box::new(dealing)).encode();

    let commitment_count = u16::from_be_bytes([encoded[2], encoded[3]]) as usize;
    // tag, dealer, commitment count, points, sealed count, dealer, recipient
    let length_at = 4 + commitment_count * 32 + 2 + 2;
    encoded[length_at..length_at + 4].copy_from_slice(&7u32.to_be_bytes());

    assert!(matches!(
        Frame::decode(&encoded),
        Err(CustodyError::Malformed(_))
    ));
}

#[test]
fn a_frame_over_the_size_bound_is_refused_without_being_parsed() {
    let oversized = vec![0x01u8; MAX_FRAME_LEN + 1];
    assert!(matches!(
        Frame::decode(&oversized),
        Err(CustodyError::Malformed(_))
    ));
}

/// Records the largest single allocation made on a thread that asked to be
/// watched. Thread-scoped because the other tests in this file run in parallel
/// and allocate what they like.
struct Watching;

static LARGEST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

thread_local! {
    static WATCHED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged, so `System`'s guarantees are this allocator's; the only addition
// is an atomic maximum and a thread-local read, neither of which allocates.
unsafe impl std::alloc::GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if WATCHED.with(std::cell::Cell::get) {
            LARGEST.fetch_max(layout.size(), std::sync::atomic::Ordering::Relaxed);
        }
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: `ptr` came from `alloc` above, i.e. from `System`.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Watching = Watching;

#[test]
fn a_huge_claimed_count_in_a_tiny_frame_is_refused_without_a_large_reservation() {
    // Tag, dealer, and a u16 commitment count of 65,535 -- then nothing. The
    // decoder used to reserve for the claimed count (2 MiB of points) before
    // noticing the bytes were not there. Refusal was always the outcome; what
    // this pins is that refusing costs nothing.
    let frame = [0x02u8, 1, 0xFF, 0xFF];

    WATCHED.with(|w| w.set(true));
    let decoded = Frame::decode(&frame);
    WATCHED.with(|w| w.set(false));

    assert!(matches!(decoded, Err(CustodyError::Malformed(_))));
    let largest = LARGEST.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        largest < 4096,
        "decoding a 4-byte frame reserved {largest} bytes at once"
    );
}
