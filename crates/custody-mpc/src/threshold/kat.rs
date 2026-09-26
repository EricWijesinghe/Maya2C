//! The known-answer test: this port against the authors' reference, bit for
//! bit.
//!
//! `scripts/traccoon_vectors.py` ran the reference (`masksign/ec24-thrc`,
//! `thrc-py` at `8ef114a`) through a 3-of-5 session -- parties 0, 2 and 4
//! sign, 1 and 3 are offline -- and recorded a digest of every step. The
//! reference's Gaussian samples are replayed in (see [`super::gauss`] for
//! why); everything else is recomputed here and must match exactly: `A`, the
//! key, all five shares, 25 pairwise seeds, each commitment, mask, MAC and
//! response, and the signature.
//!
//! A unit test rather than an integration test because it drives
//! [`sign_1_reference`], the reference's own nonce derivation, which is
//! crate-private: the public [`super::protocol::sign_1`] binds the nonce to
//! the session and so, deliberately, does not reproduce these bytes.

use super::digest;
use super::gauss::Noise;
use super::params::{KEY_LEN, MU_LEN, Q, Q_W, SID_LEN, b2};
use super::protocol::{
    KeyShare, VerifyingKey, combine, keygen_dealer, sign_1_reference, sign_2, sign_3, verify,
};
use super::ring;

const FIXTURE: &str = include_str!("../../tests/fixtures/traccoon_kat.json");
const SAMPLES: &[u8] = include_bytes!("../../tests/fixtures/traccoon_kat_samples.bin");
const SAMPLE_BYTES: usize = 6;

/// Plays back the reference's samples, and checks it is asked for them with
/// the same seed and width the reference used.
struct Replay {
    seeds: Vec<Vec<u8>>,
    widths: Vec<f64>,
    next: usize,
}

impl Noise for Replay {
    fn rounded(&mut self, sigma2: f64, seed: &[u8]) -> Vec<i64> {
        let i = self.next;
        assert_eq!(
            hex::encode(seed),
            hex::encode(&self.seeds[i]),
            "sample {i}: seed"
        );
        assert_eq!(
            sigma2.to_bits(),
            self.widths[i].to_bits(),
            "sample {i}: width"
        );
        self.next += 1;
        SAMPLES[i * 512 * SAMPLE_BYTES..(i + 1) * 512 * SAMPLE_BYTES]
            .as_chunks::<SAMPLE_BYTES>()
            .0
            .iter()
            .map(|b| {
                let mut x = [0u8; 8];
                x[..SAMPLE_BYTES].copy_from_slice(b);
                // Sign-extend the 48-bit two's complement value.
                (i64::from_le_bytes(x) << 16) >> 16
            })
            .collect()
    }
}

fn kat() -> serde_json::Value {
    serde_json::from_str(FIXTURE).expect("fixture")
}

fn bytes<const L: usize>(v: &serde_json::Value) -> [u8; L] {
    hex::decode(v.as_str().expect("hex"))
        .expect("hex")
        .try_into()
        .expect("length")
}

fn hex_of(d: [u8; 32]) -> String {
    hex::encode(d)
}

/// The reference digests `h` as centred values reduced mod `Q`.
fn digest_hint(h: &[ring::Poly]) -> String {
    let lifted: Vec<ring::Poly> = h
        .iter()
        .map(|p| {
            ring::centered(p, Q_W)
                .into_iter()
                .map(|x| u64::try_from(x.rem_euclid(i128::from(Q))).expect("below Q"))
                .collect()
        })
        .collect();
    hex_of(digest(&lifted))
}

fn replay_from(kat: &serde_json::Value) -> Replay {
    let strings = |key: &str| -> Vec<String> {
        kat[key]
            .as_array()
            .expect("array")
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect()
    };
    Replay {
        seeds: strings("sample_seeds")
            .iter()
            .map(|s| hex::decode(s).expect("hex"))
            .collect(),
        widths: strings("sample_sig2")
            .iter()
            .map(|s| s.parse().expect("f64"))
            .collect(),
        next: 0,
    }
}

fn expect_str<'a>(kat: &'a serde_json::Value, key: &str, party: usize) -> &'a str {
    kat[key][party.to_string()]
        .as_str()
        .expect("fixture string")
}

/// `A`, the key, all five shares and all 25 pairwise seeds.
fn assert_keygen_matches(kat: &serde_json::Value, vk: &VerifyingKey, shares: &[KeyShare]) {
    assert_eq!(
        hex::encode(vk.a_seed),
        kat["a_seed"].as_str().expect("a_seed")
    );
    assert_eq!(hex_of(digest(&vk.t)), kat["t"].as_str().expect("t"));
    for share in shares {
        let i = share.index();
        assert_eq!(
            hex_of(share.share_digest()),
            expect_str(kat, "shares", i),
            "share {i}"
        );
        for j in 0..5 {
            let expected = kat["pair_seeds"][format!("{i},{j}")]
                .as_str()
                .expect("seed");
            assert_eq!(
                hex::encode(share.pair_seed(j).expect("seed")),
                expected,
                "seed {i},{j}"
            );
        }
    }
}

#[test]
fn the_port_reproduces_the_authors_reference_bit_for_bit() {
    let kat = kat();
    let draws: Vec<[u8; KEY_LEN]> = kat["random_draws"]
        .as_array()
        .expect("draws")
        .iter()
        .map(bytes::<KEY_LEN>)
        .collect();
    let mut replay = replay_from(&kat);
    let act = [0usize, 2, 4];
    let mu: [u8; MU_LEN] = bytes(&kat["mu"]);
    let sid: [u8; SID_LEN] = bytes(&kat["sid"]);

    let (vk, mut shares) = keygen_dealer(&draws[0], 3, 5, &mut replay).expect("keygen");
    assert_keygen_matches(&kat, &vk, &shares);

    let mut sessions = Vec::new();
    let mut first = Vec::new();
    for (pos, &j) in act.iter().enumerate() {
        let (session, r1) = sign_1_reference(
            &vk,
            &mut shares[j],
            &sid,
            &act,
            &mu,
            &draws[1 + pos],
            &mut replay,
        )
        .expect("round 1");
        assert_eq!(hex::encode(r1.cmt), expect_str(&kat, "cmt", j), "cmt {j}");
        assert_eq!(
            hex_of(digest(&r1.mask)),
            expect_str(&kat, "m", j),
            "mask {j}"
        );
        sessions.push(session);
        first.push(r1);
    }
    assert_eq!(
        replay.next, 36,
        "every recorded sample was asked for, in order"
    );

    let mut second = Vec::new();
    for (session, &j) in sessions.iter_mut().zip(&act) {
        let r2 = sign_2(session, &shares[j], &first).expect("round 2");
        assert_eq!(hex_of(digest(&r2.w)), expect_str(&kat, "w", j), "w {j}");
        let macs: Vec<String> = r2.macs.iter().map(hex::encode).collect();
        let expected: Vec<String> = kat["macs"][j.to_string()]
            .as_array()
            .expect("macs")
            .iter()
            .map(|m| m.as_str().expect("mac").to_string())
            .collect();
        assert_eq!(macs, expected, "macs {j}");
        second.push(r2);
    }
    let mut third = Vec::new();
    for (session, &j) in sessions.iter_mut().zip(&act) {
        let z = sign_3(session, &shares[j], &second).expect("round 3");
        assert_eq!(hex_of(digest(&z)), expect_str(&kat, "z_share", j), "z {j}");
        third.push(z);
    }

    let signature = combine(&vk, &mu, &act, &first, &second, &third).expect("combine");
    assert_eq!(
        hex::encode(signature.c_hash),
        kat["sig_c"].as_str().expect("c")
    );
    assert_eq!(
        hex_of(digest(&signature.z)),
        kat["sig_z"].as_str().expect("z")
    );
    assert_eq!(digest_hint(&signature.h), kat["sig_h"].as_str().expect("h"));
    assert!(verify(&vk, &mu, &signature));
    assert!(!verify(&vk, &[0u8; MU_LEN], &signature), "another message");

    let reference_b2: f64 = kat["b2"].as_str().expect("b2").parse().expect("f64");
    assert!((b2() / reference_b2 - 1.0).abs() < 1e-12);
}
