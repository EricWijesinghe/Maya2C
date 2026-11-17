//! Stealth scanning cost per announcement — ADR-102 "honest cost note".
//!
//! Measures `ViewKey::scan` on one core for two populations: foreign
//! announcements (another recipient's, the common case: one decapsulation, then
//! the tag stops it about 255 times in 256) and own announcements (decapsulation,
//! derivation and the output search). Plain `Instant` timing like
//! `benches/certificate.rs`; prints the figure, never compares it to a target.
//!
//! `cargo bench -p maya-crypto-pq --bench stealth_scan`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_crypto_pq::stealth::{ScanSeed, StealthPayment, ViewKey};

const N: usize = 2_000;

fn view(n: u8) -> ViewKey {
    ViewKey::from_seed(&ScanSeed::from_bytes([n; 64]), [n; 32])
}

fn time_scan(scanner: &ViewKey, payments: &[StealthPayment]) -> (f64, usize) {
    let mut hits = 0;
    let t = Instant::now();
    for p in payments {
        if scanner
            .scan(&p.announcement, &[p.one_time_address])
            .is_some()
        {
            hits += 1;
        }
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / payments.len() as f64;
    (us, hits)
}

fn main() {
    let (alice, bob) = (view(1), view(2));
    let meta = alice.meta_address();
    let payments: Vec<_> = (0..N).map(|_| meta.pay()).collect();

    // Warm-up pass so the first timed loop does not pay for page faults.
    let _ = time_scan(&bob, &payments[..100]);

    let (foreign_us, foreign_hits) = time_scan(&bob, &payments);
    let (own_us, own_hits) = time_scan(&alice, &payments);
    assert_eq!(foreign_hits, 0, "a wrong view key detected a payment");
    assert_eq!(own_hits, N, "the right view key missed a payment");

    println!("stealth scan, ML-KEM-768, one core, {N} announcements each:");
    println!(
        "  foreign (not ours): {foreign_us:.2} us/announcement ({:.0}/s)",
        1e6 / foreign_us
    );
    println!(
        "  own (detected):     {own_us:.2} us/announcement ({:.0}/s)",
        1e6 / own_us
    );
}
