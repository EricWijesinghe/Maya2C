//! Property tests for the wallet's untrusted-input parsers: `maya:` payment
//! URIs (QR codes and pasted text), BIP-32-style derivation paths, and
//! air-gap QR frames.
//!
//! A seeded xorshift generator stands in for proptest so the crate's
//! dev-dependencies stay as they are; every case is reproducible from its
//! printed seed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_wallet_core::airgap::{Assembler, Frame, split};
use maya_wallet_core::hd::DerivationPath;
use maya_wallet_core::payment::PaymentRequest;

const CASES: u64 = 2_000;

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
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// Fragments that steer random strings into the URI grammar's edges.
const URI_PARTS: &[&str] = &[
    "maya:",
    "?",
    "&",
    "=",
    "amount",
    "label",
    "message",
    "exp",
    "id",
    "%",
    "%2",
    "%zz",
    "%00",
    "%FF",
    "%C3%A9",
    "0",
    "1",
    "18446744073709551616",
    "-1",
    "1e9",
    " ",
    "\u{202e}",
    "\u{0}",
    "maya1",
    "tmaya1",
    "0x",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "115792089237316195423570985008687907853269984665640564039457584007913129639936",
];

fn random_uri(rng: &mut Rng) -> String {
    // Half the cases carry a valid address so the query parser is reached.
    let mut s = if rng.below(2) == 0 {
        format!("maya:{}?", "ab".repeat(32))
    } else {
        String::new()
    };
    for _ in 0..rng.below(12) {
        if rng.below(4) == 0 {
            s.push_str(&String::from_utf8_lossy(&rng.bytes(8)));
        } else {
            s.push_str(rng.pick(URI_PARTS));
        }
    }
    s
}

#[test]
fn payment_uri_parse_never_panics() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..CASES {
        let input = random_uri(&mut rng);
        let _ = PaymentRequest::parse(&input);
    }
}

#[test]
fn payment_uri_duplicate_key_is_always_refused() {
    let address = "ab".repeat(32);
    for key in ["amount", "label", "message", "exp", "id", "unknown"] {
        let uri = format!("maya:{address}?{key}=1&{key}=2");
        assert!(
            PaymentRequest::parse(&uri).is_err(),
            "duplicate {key} accepted"
        );
    }
}

#[test]
fn payment_uri_amount_round_trips_any_u64() {
    let address = "cd".repeat(32);
    let mut rng = Rng(7);
    for _ in 0..CASES {
        let value = rng.next();
        let req = PaymentRequest::parse(&format!("maya:{address}?amount={value}")).unwrap();
        assert_eq!(req.amount, Some(primitive_types::U256::from(value)));
    }
}

#[test]
fn derivation_path_parse_never_panics_and_display_round_trips() {
    const PARTS: &[&str] = &[
        "m",
        "/",
        "'",
        "h",
        "0",
        "1",
        "44",
        "2147483647",
        "2147483648",
        "4294967295",
        "4294967296",
        "-",
        "+1",
        " ",
        "//",
        "x",
    ];
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..CASES {
        let mut path = String::new();
        for _ in 0..rng.below(10) {
            path.push_str(rng.pick(PARTS));
        }
        if let Ok(parsed) = DerivationPath::parse(&path) {
            let again = DerivationPath::parse(&parsed.to_string())
                .unwrap_or_else(|e| panic!("display of {path:?} does not re-parse: {e}"));
            assert_eq!(again, parsed, "round trip changed {path:?}");
        }
    }
}

#[test]
fn airgap_frame_decode_never_panics_and_re_encodes_exactly() {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    for _ in 0..CASES {
        let mut bytes = rng.bytes(96);
        // Half the cases start from a well-formed frame so mutations reach
        // the field checks, not only the magic check.
        if rng.below(2) == 0 {
            let payload = rng.bytes(64);
            if let Ok(frames) = split(&[payload, vec![1]].concat()) {
                bytes = frames[0].encode();
                let at = rng.below(bytes.len());
                bytes[at] ^= rng.next() as u8;
            }
        }
        if let Ok(frame) = Frame::decode(&bytes) {
            assert!(frame.index < frame.total);
            assert_eq!(frame.encode(), bytes, "decode/encode is not canonical");
        }
    }
}

#[test]
fn airgap_reassembles_in_any_order_with_duplicates() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for _ in 0..200 {
        let mut payload = rng.bytes(4_000);
        payload.push(0xEE);
        let frames = split(&payload).unwrap();
        let first = Frame::decode(&frames[rng.below(frames.len())].encode()).unwrap();
        let mut asm = Assembler::new(&first);
        for _ in 0..frames.len() * 3 {
            let wire = frames[rng.below(frames.len())].encode();
            asm.add(&Frame::decode(&wire).unwrap()).unwrap();
        }
        for frame in &frames {
            asm.add(frame).unwrap();
        }
        assert_eq!(asm.finish().unwrap(), payload);
    }
}

#[test]
fn airgap_refuses_frames_from_another_payload() {
    let a = split(&[1u8; 900]).unwrap();
    let b = split(&[2u8; 900]).unwrap();
    let mut asm = Assembler::new(&a[0]);
    assert!(asm.add(&b[0]).is_err());
}
