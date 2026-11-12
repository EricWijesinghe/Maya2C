//! The WGSL source, with every number written in from `params.rs` and
//! `lattice.rs` so the shader cannot hold a constant the CPU does not.
//!
//! Not behind the `gpu` feature: the source scan in the tests runs on every
//! machine.

use std::fmt::Write as _;

use crate::lattice::{HEADER_WORDS, WORDS_IN, WORDS_OUT};
use crate::params::{F_MONT, K, L, N, NEG_QINV, Q, R2, ZETAS_MONT};

/// The shader body, without its constants.
pub const BODY: &str = include_str!("shaders/lattice.wgsl");

/// The entry point's name.
pub const ENTRY_POINT: &str = "lattice";

/// The complete shader.
#[must_use]
pub fn source() -> String {
    let constants = [
        ("Q", Q as usize),
        ("NEG_QINV", NEG_QINV as usize),
        ("R2", R2 as usize),
        ("F_MONT", F_MONT as usize),
        ("K", K),
        ("L", L),
        ("N", N),
        ("WORDS_IN", WORDS_IN),
        ("WORDS_OUT", WORDS_OUT),
        ("IN_Z", 0),
        ("IN_C", L * N),
        ("IN_T1", (L + 1) * N),
        ("IN_A_HAT", (L + 1 + K) * N),
        ("OUT_Z_HAT", HEADER_WORDS),
        ("OUT_C_HAT", HEADER_WORDS + L * N),
        ("OUT_T1_HAT", HEADER_WORDS + (L + 1) * N),
        ("OUT_W_APPROX", HEADER_WORDS + (L + 1 + K) * N),
    ];
    let mut out = String::new();
    for (name, value) in constants {
        let _ = writeln!(out, "const {name}: u32 = {value}u;");
    }
    let zetas: Vec<String> = ZETAS_MONT.iter().map(|z| format!("{z}u")).collect();
    let _ = writeln!(
        out,
        "var<private> ZETAS: array<u32, {N}> = array<u32, {N}>({});",
        zetas.join(", ")
    );
    out.push_str(BODY);
    out
}
