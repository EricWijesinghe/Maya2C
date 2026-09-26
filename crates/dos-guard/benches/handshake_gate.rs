//! How many handshakes a node can reject versus accept per second
//! (Master Prompt 16 §4). Reject = a cookie check that fails (one keyed
//! BLAKE3). Accept = a cookie check that passes plus the responder's ML-KEM-768
//! encapsulation. Single core. `cargo bench -p maya-dos-guard --bench handshake_gate`

#![allow(
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use std::time::Instant;

use maya_crypto_pq::kem::{EncapsulationKey, generate_keypair};
use maya_dos_guard::Addr;
use maya_dos_guard::cookie::CookieJar;
use maya_dos_guard::puzzle;

fn rate(n: u64, mut f: impl FnMut(u64)) -> f64 {
    let t = Instant::now();
    for i in 0..n {
        f(i);
    }
    n as f64 / t.elapsed().as_secs_f64()
}

fn main() {
    let jar = CookieJar::new([9; 32]);
    let now = 1_000_000;
    let reject = rate(1_000_000, |i| {
        let a = Addr::V4([10, 0, (i >> 8) as u8, i as u8]);
        std::hint::black_box(jar.check(a, &[0; 16], now));
    });
    let (_, ek) = generate_keypair();
    let ek = EncapsulationKey::from_bytes(&ek.to_bytes()).unwrap();
    let accept = rate(20_000, |i| {
        let a = Addr::V4([10, 0, (i >> 8) as u8, i as u8]);
        let c = jar.issue(a, now);
        if jar.check(a, &c, now) {
            std::hint::black_box(ek.encapsulate());
        }
    });
    println!("handshake gate, one core:");
    println!("  reject (bad cookie)              {reject:>12.0} /s");
    println!("  accept (cookie + ML-KEM encaps)  {accept:>12.0} /s");
    println!(
        "  ratio                            {:>12.0}x",
        reject / accept
    );
    for bits in [8u32, 12, 16] {
        let t = Instant::now();
        let mut attempts = 0;
        for c in 0..16u8 {
            attempts += puzzle::solve(&[c; 32], bits).1;
        }
        println!(
            "  puzzle {bits:>2} bits: {:>8.0} hashes avg, {:>7.2} ms avg to solve",
            attempts as f64 / 16.0,
            t.elapsed().as_secs_f64() * 1e3 / 16.0
        );
    }
}
