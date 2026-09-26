//! Differential test: `split` against the Lean model's evaluated vectors
//! (`formal/lean/Maya2C/FeeSplit.lean`, whose `split_sums` theorem proves the
//! model sums to exactly what was paid). If the Rust and the model ever
//! disagree on one line, the proof no longer describes this code.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_fee_market::split;

#[test]
fn split_matches_the_lean_model_on_every_vector() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../formal/lean/vectors/fee_split.txt");
    let text = std::fs::read_to_string(path).expect("vectors are committed");
    let mut checked = 0;
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let v: Vec<u64> = line.split_whitespace().map(|x| x.parse().unwrap()).collect();
        let (base, tip, bps) = (v[0], v[1], v[2]);
        let s = split(base, tip, bps);
        assert_eq!((s.burned, s.treasury, s.tip), (v[3], v[4], v[5]), "base {base} bps {bps}");
        checked += 1;
    }
    assert_eq!(checked, 49);
}
