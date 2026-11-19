//! Property tests for chat decoders that read bytes from the network: the
//! relay envelope (CBOR) and the delivery-receipt plaintext.
//!
//! A seeded xorshift generator stands in for proptest so the crate's
//! dev-dependencies stay as they are; every case is reproducible.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_chat::receipt::{self, MAX_IDS};
use maya_chat::relay::Envelope;

const CASES: u64 = 3_000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let len = self.below(max + 1);
        (0..len).map(|_| self.next() as u8).collect()
    }
}

#[test]
fn envelope_decode_never_panics_on_random_bytes() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..CASES {
        let _ = Envelope::decode(&rng.bytes(256));
    }
}

/// CBOR headers that declare huge lengths with no body behind them: a decoder
/// that pre-allocates from the header would abort on these.
#[test]
fn envelope_decode_refuses_hostile_length_headers() {
    let hostile: &[&[u8]] = &[
        &[0x5B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], // bytes, 2^64-1
        &[0x7B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], // text, 2^64-1
        &[0x9B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], // array, 2^64-1
        &[0xBB, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], // map, 2^64-1
        &[0xBA, 0xFF, 0xFF, 0xFF, 0xFF, 0x67, b'v', b'e', b'r', b's', b'i', b'o', b'n'],
        &[0x9F; 4096], // nested indefinite arrays
        &[0x81; 4096], // nested definite arrays
        &[0xC6; 4096], // nested tags
    ];
    for bytes in hostile {
        assert!(Envelope::decode(bytes).is_err());
    }
}

#[test]
fn envelope_decode_survives_mutated_cbor_maps() {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    for _ in 0..CASES {
        // A map header with plausible text keys, then random tail bytes.
        let mut bytes = vec![0xA0 | (rng.below(8) as u8)];
        for key in ["version", "to", "from", "body", "nonce", "sig", "ts"] {
            if rng.below(2) == 0 {
                bytes.push(0x60 | key.len() as u8);
                bytes.extend_from_slice(key.as_bytes());
                bytes.extend(rng.bytes(12));
            }
        }
        let _ = Envelope::decode(&bytes);
    }
}

#[test]
fn receipt_parse_never_panics_and_respects_bounds() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..CASES {
        let mut bytes = receipt::encode(&vec![[7; 32]; 1 + rng.below(4)], &rng.bytes(16));
        // Corrupt the count, truncate, or extend.
        match rng.below(3) {
            0 => {
                let at = rng.below(bytes.len());
                bytes[at] = rng.next() as u8;
            }
            1 => bytes.truncate(rng.below(bytes.len())),
            _ => bytes.extend(rng.bytes(64)),
        }
        if let Ok((ids, sig)) = receipt::parse(&bytes) {
            assert!(!ids.is_empty() && ids.len() <= MAX_IDS);
            assert!(sig.len() <= bytes.len());
        }
        let _ = receipt::parse(&rng.bytes(80));
    }
}

#[test]
fn receipt_round_trips_any_count_up_to_the_cap() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for count in [1, 2, 17, MAX_IDS - 1, MAX_IDS] {
        let ids: Vec<[u8; 32]> = (0..count)
            .map(|_| core::array::from_fn(|_| rng.next() as u8))
            .collect();
        let sig = rng.bytes(64);
        let bytes = receipt::encode(&ids, &sig);
        match receipt::parse(&bytes) {
            Ok((got, got_sig)) => {
                assert_eq!(got, ids);
                assert_eq!(got_sig, &sig[..]);
            }
            // An empty signature may be refused by the shape check; anything
            // else must parse.
            Err(e) => assert!(sig.is_empty(), "count {count} refused: {e:?}"),
        }
    }
    assert!(receipt::parse(&receipt::encode(&vec![[0; 32]; MAX_IDS + 1], &[1])).is_err());
}
