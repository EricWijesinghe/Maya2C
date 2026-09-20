//! APDU integration against the Speculos emulator.
//!
//! =========================================================================
//! NOT RUN. NOT RUNNABLE WHERE THIS WAS WRITTEN.
//! =========================================================================
//!
//! Speculos is absent, Docker is absent, there is no `arm-none-eabi-gcc`, and
//! the Ledger C SDK is not checked out. Every test here is `#[ignore]` so a
//! plain `cargo test` reports them as skipped rather than failing on a
//! connection refused — and so nobody reads a green suite as evidence the
//! device works.
//!
//! There is a second reason they cannot pass yet, and it is not environmental:
//! **the app has no signer.** `SIGN_TRANSACTION` returns `0x6A81` because a
//! Maya2C signature needs both an ML-DSA-65 and an SLH-DSA half, and whether
//! the hash-based half fits on the device is the open question this crate
//! exists to answer. See `docs/ledger-feasibility.md`.
//!
//! Run them, once there is something to run:
//!
//! ```bash
//! speculos --model nanosp --display headless \
//!     target/nanosplus/release/app-maya2c &
//! cargo test --test ledger_tests -- --ignored
//! ```
//!
//! # What these check that the host tests cannot
//!
//! `tests/apdu_tests.rs` covers the codec exhaustively and needs no device.
//! What it cannot cover is the transport: whether a 44-page response actually
//! survives the USB/HID layer, whether the device's own buffering agrees with
//! `MAX_APDU_PAYLOAD`, and whether an interrupted sequence leaves the device in
//! a state the next command can recover from. Those are the tests below.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::TcpStream;

use app_maya2c::apdu::{CLA, HYBRID_PUBLIC_KEY_LEN, MAX_APDU_PAYLOAD, Pages};

/// Speculos' default APDU port.
const SPECULOS_APDU: &str = "127.0.0.1:9999";

/// Sends one APDU and returns `(payload, status_word)`.
///
/// Speculos' TCP protocol is a 4-byte big-endian length then the frame; the
/// reply is the same, with the last two bytes being the status word.
fn exchange(frame: &[u8]) -> std::io::Result<(Vec<u8>, u16)> {
    let mut stream = TcpStream::connect(SPECULOS_APDU)?;

    let mut out = (frame.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(frame);
    stream.write_all(&out)?;

    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body)?;

    // A reply is always at least a status word. Anything shorter is a protocol
    // violation rather than an empty response.
    assert!(body.len() >= 2, "reply shorter than a status word");
    let status = u16::from_be_bytes([body[body.len() - 2], body[body.len() - 1]]);
    body.truncate(body.len() - 2);
    Ok((body, status))
}

fn frame(ins: u8, p1: u8, p2: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![CLA, ins, p1, p2, payload.len() as u8];
    out.extend_from_slice(payload);
    out
}

/// The standard path, in wire form.
fn path() -> Vec<u8> {
    let components: [u32; 5] = [
        44 | 0x8000_0000,
        7331 | 0x8000_0000,
        0x8000_0000,
        0x8000_0000,
        0x8000_0000,
    ];
    let mut out = vec![5u8];
    for c in components {
        out.extend_from_slice(&c.to_be_bytes());
    }
    out
}

#[test]
#[ignore = "needs Speculos; see the module docs"]
fn the_device_returns_a_full_public_key() {
    // 1,984 bytes over a 255-byte pipe: eight responses. The point is that the
    // paging survives the transport, not that the key is any particular value.
    let (payload, status) = exchange(&frame(0x02, 0x00, 0x00, &path())).expect("speculos");

    assert_eq!(status, 0x9000);
    assert_eq!(payload.len(), HYBRID_PUBLIC_KEY_LEN);
    assert_eq!(Pages::count_for(payload.len()), 8);
}

#[test]
#[ignore = "needs Speculos; see the module docs"]
fn a_foreign_coin_type_is_refused_by_the_device() {
    // The host tests prove the parser refuses it. This proves the device does
    // -- that the check is actually on the path a command takes, and not
    // bypassed by the SDK's own dispatch.
    let mut bitcoin = vec![5u8];
    for c in [
        44 | 0x8000_0000u32,
        0x8000_0000,
        0x8000_0000,
        0x8000_0000,
        0x8000_0000,
    ] {
        bitcoin.extend_from_slice(&c.to_be_bytes());
    }

    let (_, status) = exchange(&frame(0x02, 0x00, 0x00, &bitcoin)).expect("speculos");
    assert_eq!(status, 0x6A80, "expected BadDerivationPath");
}

#[test]
#[ignore = "needs Speculos; see the module docs"]
fn an_interrupted_sequence_leaves_the_device_recoverable() {
    // The failure this catches is a device that has to be unplugged. Start a
    // sequence, abandon it, and check a fresh one still works.
    let payload = [0u8; MAX_APDU_PAYLOAD];

    let (_, status) = exchange(&frame(0x04, 0x00, 0x00, &payload)).expect("speculos");
    assert_eq!(status, 0x9000, "first chunk accepted");

    // Skip index 1 entirely and jump to 2. The device must refuse and reset.
    let (_, status) = exchange(&frame(0x04, 0x01, 0x02, &payload)).expect("speculos");
    assert_eq!(status, 0x6985, "out-of-order chunk refused");

    // A new sequence must start cleanly rather than continuing the old one.
    let (_, status) = exchange(&frame(0x04, 0x00, 0x00, &payload)).expect("speculos");
    assert_eq!(status, 0x9000, "the device recovered without a reconnect");
}

#[test]
#[ignore = "needs Speculos, and a signer that fits; see docs/ledger-feasibility.md"]
fn a_transaction_is_signed() {
    // Cannot pass today, and not only for want of an emulator: the app has no
    // signer. Written so that the day one exists, the test is already here and
    // says what it expects -- a *hybrid* signature, not a lattice half.
    use app_maya2c::apdu::HYBRID_SIGNATURE_LEN;

    let transaction = vec![0xAAu8; 4096];
    let pages: Vec<&[u8]> = Pages::new(&transaction).collect();
    let last = pages.len() - 1;

    for (index, page) in pages.iter().enumerate() {
        let p1 = if index == 0 {
            0x00
        } else if index == last {
            0x02
        } else {
            0x01
        };
        let (_, status) = exchange(&frame(0x04, p1, index as u8, page)).expect("speculos");
        if index < last {
            assert_eq!(status, 0x9000, "chunk {index}");
        } else {
            assert_eq!(status, 0x9000, "final chunk");
        }
    }

    // The assertion that makes this test worth having. A device returning 3,309
    // bytes has produced an ML-DSA-65 signature, which is not a Maya2C
    // signature and which the chain rejects.
    let (signature, status) = exchange(&frame(0x04, 0x02, 0x00, &[])).expect("speculos");
    assert_eq!(status, 0x9000);
    assert_eq!(
        signature.len(),
        HYBRID_SIGNATURE_LEN,
        "a lattice-only signature is not a valid Maya2C signature"
    );
}
