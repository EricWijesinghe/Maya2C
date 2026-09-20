//! ML-KEM-768 transport handshake and frame-sealing benchmark.
//!
//! Answers the two questions the post-quantum transport has to survive:
//!
//! 1. **What does a connection cost?** Every inbound and outbound connection
//!    runs an ML-KEM-768 exchange, so this is the price of joining a mesh and
//!    the price of every rotation. A node with fifty peers pays it fifty times
//!    per epoch.
//! 2. **What does a byte cost?** Every gossip byte is sealed twice now — once
//!    by Noise underneath, once by this layer. If the second seal were
//!    expensive, block propagation would pay for it continuously rather than
//!    once per connection.
//!
//! Run with:
//! ```text
//! cargo bench --bench mlkem_handshake
//! ```
//!
//! The figures to compare against: a 15 s target block time, and a 13215-byte
//! hybrid-signed transaction.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use maya_crypto_pq::kem::{self, HANDSHAKE_OVERHEAD};

/// A block-sized payload, for the sealing throughput group.
///
/// 64 transactions at 13215 bytes: the same block shape
/// `benches/hybrid_footprint.rs` measures, so the two are comparable.
const BLOCK_BYTES: usize = 64 * 13_215;

fn bench_key_exchange(c: &mut Criterion) {
    let (decapsulation_key, encapsulation_key) = kem::generate_keypair();
    let encoded = encapsulation_key.to_bytes();
    let (ciphertext, _) = encapsulation_key.encapsulate();

    let mut group = c.benchmark_group("mlkem768");

    // What the listener pays per inbound connection.
    group.bench_function("generate_keypair", |b| {
        b.iter(kem::generate_keypair);
    });

    // What the dialer pays. Includes decoding the peer's key, because a real
    // dialer always does — and because that decode is the one step in the
    // exchange that can legitimately fail.
    group.bench_function("decode_and_encapsulate", |b| {
        b.iter(|| {
            kem::EncapsulationKey::from_bytes(black_box(&encoded))
                .expect("decode")
                .encapsulate()
        });
    });

    group.bench_function("encapsulate", |b| {
        b.iter(|| encapsulation_key.encapsulate());
    });

    group.bench_function("decapsulate", |b| {
        b.iter(|| decapsulation_key.decapsulate(black_box(&ciphertext)));
    });

    // The whole exchange, as one connection experiences it: the responder
    // generates, the initiator decodes and encapsulates, the responder
    // decapsulates. This is the number that multiplies by peer count.
    group.bench_function("full_connection_handshake", |b| {
        b.iter(|| {
            let (dk, ek) = kem::generate_keypair();
            let encoded = ek.to_bytes();
            let peer = kem::EncapsulationKey::from_bytes(&encoded).expect("decode");
            let (ct, _sender) = peer.encapsulate();
            black_box(dk.decapsulate(&ct))
        });
    });

    group.finish();

    println!("\nhandshake adds {HANDSHAKE_OVERHEAD} bytes per connection");
}

fn bench_frame_sealing(c: &mut Criterion) {
    // The per-byte cost, which block propagation pays continuously. Measured on
    // the AEAD directly rather than through `PqStream`, so the number is the
    // cryptography rather than the framing bookkeeping around it.
    let cipher = ChaCha20Poly1305::new(&[7u8; 32].into());
    let nonce = Nonce::from([0u8; 12]);

    let mut group = c.benchmark_group("frame_sealing");
    group.throughput(Throughput::Bytes(BLOCK_BYTES as u64));

    let payload = vec![0xABu8; BLOCK_BYTES];

    group.bench_function("seal_block", |b| {
        b.iter(|| {
            let mut buffer = payload.clone();
            cipher
                .encrypt_in_place(&nonce, b"", &mut buffer)
                .expect("seal");
            black_box(buffer)
        });
    });

    let mut sealed = payload.clone();
    cipher
        .encrypt_in_place(&nonce, b"", &mut sealed)
        .expect("seal");

    group.bench_function("open_block", |b| {
        b.iter(|| {
            let mut buffer = sealed.clone();
            cipher
                .decrypt_in_place(&nonce, b"", &mut buffer)
                .expect("open");
            black_box(buffer)
        });
    });

    group.finish();

    println!("\nblock payload measured: {BLOCK_BYTES} bytes (64 hybrid-signed transfers)");
}

criterion_group!(benches, bench_key_exchange, bench_frame_sealing);
criterion_main!(benches);
