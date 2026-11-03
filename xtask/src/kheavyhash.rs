//! `cargo xtask kheavyhash [--fetch]` — regenerates the kHeavyHash reference
//! module and its known-answer fixture from rusty-kaspa at one pinned commit.
//!
//! The sponge initial states and the test vectors are copied from upstream by
//! this command, digit for digit, so they are never retyped. The benches and
//! the reference tests (`crates/node/tests/kheavyhash_reference_tests.rs`) check
//! the result three ways.
//!
//! The upstream files are cached under `target/kheavyhash-src/` (gitignored with
//! the rest of `target/`). Without `--fetch` the command reads that cache, so a
//! re-run reproduces the committed outputs byte for byte with no network. With
//! `--fetch` it downloads the two files again from the pinned commit first.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::Client;

const COMMIT: &str = "01b532e8b553523216471682649693af92f0fd16";
const RAW_BASE: &str = "https://raw.githubusercontent.com/kaspanet/rusty-kaspa/";
const POW_HASHERS: &str = "crypto/hashes/src/pow_hashers.rs";
const MATRIX: &str = "consensus/pow/src/matrix.rs";
const MODULE_OUT: &str = "crates/node/benches/support/kheavyhash.rs";
const FIXTURE_OUT: &str = "crates/node/tests/fixtures/kaspa_kheavyhash.rs";
const CACHE: &str = "target/kheavyhash-src";
const INITIAL_STATE_MARK: &str = "INITIAL_STATE: [u64; 25] = [";

pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [] => {}
        [flag] if flag == "--fetch" => fetch(&cache_dir())?,
        _ => return Err("usage: cargo xtask kheavyhash [--fetch]".into()),
    }
    let cache = cache_dir();
    let hashers = read_cached(&cache, POW_HASHERS)?;
    let matrix = read_cached(&cache, MATRIX)?;
    let vectors = Vectors::extract(&matrix)?;

    let root = crate::workspace_root();
    let module = MODULE_TEMPLATE
        .replace("@COMMIT@", COMMIT)
        .replace(
            "@POW_STATE@",
            &fmt_words(&initial_state(&hashers, "PowHash")?),
        )
        .replace(
            "@HEAVY_STATE@",
            &fmt_words(&initial_state(&hashers, "KHeavyHash")?),
        );
    let fixture = FIXTURE_TEMPLATE
        .replace("@COMMIT@", COMMIT)
        .replace("@HH_INPUT@", &vectors.hh_input.join(", "))
        .replace("@EXPECTED@", &vectors.expected.join(", "))
        .replace("@HH_MATRIX@", &rows(&vectors.hh_matrix))
        .replace("@GEN_MATRIX@", &rows(&vectors.gen_matrix));

    let module_path = root.join(MODULE_OUT);
    let fixture_path = root.join(FIXTURE_OUT);
    write_file(&module_path, &module)?;
    write_file(&fixture_path, &fixture)?;
    // rustfmt's layout is the canonical one, so the bytes are what `cargo fmt --check` accepts.
    let status = std::process::Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(&module_path)
        .arg(&fixture_path)
        .status()
        .map_err(|e| format!("rustfmt: {e}"))?;
    if !status.success() {
        return Err(format!("rustfmt exited {status}"));
    }
    println!(
        "written; expected = {} ... input = {}",
        py_list(&vectors.expected[..4]),
        py_list(&vectors.hh_input[..4]),
    );
    Ok(())
}

fn cache_dir() -> PathBuf {
    crate::workspace_root().join(CACHE)
}

fn fetch(cache: &Path) -> Result<(), String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    for rel in [POW_HASHERS, MATRIX] {
        let url = format!("{RAW_BASE}{COMMIT}/{rel}");
        let text = client
            .get(&url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::text)
            .map_err(|e| format!("{url}: {e}"))?;
        write_file(&cache.join(rel), &text)?;
    }
    Ok(())
}

fn read_cached(cache: &Path, rel: &str) -> Result<String, String> {
    let path = cache.join(rel);
    fs::read_to_string(&path).map_err(|e| format!("{}: {e} (run with --fetch)", path.display()))
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The sponge's 25 initial words for one of `pow_hashers.rs`'s `impl` blocks.
fn initial_state(hashers: &str, block: &str) -> Result<Vec<String>, String> {
    let from_impl = &hashers[index_of(hashers, &format!("impl {block} {{"))?..];
    let after = &from_impl[index_of(from_impl, INITIAL_STATE_MARK)? + INITIAL_STATE_MARK.len()..];
    let body = &after[..index_of(after, "];")?];
    let words: Vec<String> = digit_runs(body).into_iter().map(str::to_owned).collect();
    if words.len() != 25 {
        return Err(format!(
            "{block}: expected 25 initial words, found {}",
            words.len()
        ));
    }
    Ok(words)
}

/// The known answers the fixture carries, as digit strings.
struct Vectors {
    expected: Vec<String>,
    hh_input: Vec<String>,
    hh_matrix: Vec<String>,
    gen_matrix: Vec<String>,
}

impl Vectors {
    fn extract(matrix: &str) -> Result<Self, String> {
        let test_hh = &matrix[index_of(matrix, "fn test_heavy_hash()")?..];
        let expected = numbers_after(test_hh, "let expected_hash = Hash::from_bytes([", 32)?;
        let hh_matrix = numbers_after(test_hh, "let test_matrix = Matrix([", 64 * 64)?;
        let from_input = &test_hh[index_of(test_hh, "let hash = Hash::from_bytes([")?..];
        let hh_input = numbers_after(from_input, "let hash = Hash::from_bytes([", 32)?;

        let test_gen = &matrix[index_of(matrix, "fn test_generate_matrix()")?..];
        let gen_matrix = numbers_after(test_gen, "let expected_matrix = Matrix([", 64 * 64)?;
        if !test_gen.contains("Hash::from_bytes([42; 32])") {
            return Err("test_generate_matrix no longer seeds with [42; 32]".into());
        }
        Ok(Self {
            expected,
            hh_input,
            hh_matrix,
            gen_matrix,
        })
    }
}

fn index_of(text: &str, marker: &str) -> Result<usize, String> {
    text.find(marker)
        .ok_or_else(|| format!("upstream no longer contains `{marker}`"))
}

/// Every run of ASCII digits, as Python's `re.findall(r"\d+", text)`.
fn digit_runs(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .collect()
}

fn numbers_after(text: &str, marker: &str, count: usize) -> Result<Vec<String>, String> {
    let start = index_of(text, marker)? + marker.len();
    let runs: Vec<String> = digit_runs(&text[start..])
        .into_iter()
        .take(count)
        .map(str::to_owned)
        .collect();
    if runs.len() != count {
        return Err(format!(
            "after `{marker}`: expected {count} numbers, found {}",
            runs.len()
        ));
    }
    Ok(runs)
}

/// Five words to a line, as the reference module lays out its state.
fn fmt_words(words: &[String]) -> String {
    words
        .chunks(5)
        .map(|chunk| format!("    {},", chunk.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The 64x64 matrix, one bracketed row per line.
fn rows(nums: &[String]) -> String {
    nums.chunks(64)
        .map(|row| format!("    [{}],", row.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Python's `repr` of a list of digit strings, as the script printed it.
fn py_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// The reference module. `@...@` marks what `run` fills in from upstream.
const MODULE_TEMPLATE: &str = r#"//! kHeavyHash — Kaspa's proof of work — as a **reference implementation for
//! comparison benchmarks only**. Nothing in the node links it, and it is not a
//! consensus rule here: this chain's own proof of work is `argon_blake`.
//!
//! # Where it comes from, and how it is checked
//!
//! Ported from rusty-kaspa (ISC licence, copyright the Kaspa developers) at
//! commit `@COMMIT@`: `consensus/pow/src/{matrix,xoshiro}.rs` and
//! `crypto/hashes/src/pow_hashers.rs`. The two sponge initial states below are
//! copied from upstream by `cargo xtask kheavyhash`, not retyped.
//! `crates/node/tests/kheavyhash_reference_tests.rs` then checks this module
//! three independent ways: both states against a cSHAKE256 written from NIST
//! SP 800-185 in the test itself, and upstream's own known answers for
//! `heavy_hash` and for matrix generation (`tests/fixtures/kaspa_kheavyhash.rs`).
//!
//! # Why a reference and not upstream's crate
//!
//! `kaspa-pow` does not build on this workspace's toolchain, and there is no
//! standalone `kheavyhash` crate. What a bench needs is the same arithmetic the
//! Kaspa node runs, checked by the Kaspa node's own vectors — that is what this
//! is. It is *not* a claim about how fast a Kaspa miner is: upstream uses an
//! assembly Keccak on x86-64 and miners use GPUs. `reports/02-crypto.md` says so
//! beside the number.
//!
//! # Floats
//!
//! [`Matrix::generate`] rejects rank-deficient matrices using Kaspa's own
//! `f64` Gaussian elimination, because that is the rule whose output the
//! upstream vector pins. It runs once per block template, never per nonce, and
//! never on a consensus path of this chain.

#![allow(dead_code)]
// The sponge states are upstream's decimal literals, digit for digit, so they
// can be compared with `pow_hashers.rs` by eye; separators would break that.
#![allow(clippy::unreadable_literal)]

/// A 32-byte hash, in the byte order Kaspa's `Hash` uses.
pub type Hash = [u8; 32];

fn le_words(hash: &Hash) -> [u64; 4] {
    core::array::from_fn(|i| {
        u64::from_le_bytes(hash[8 * i..8 * i + 8].try_into().expect("8 bytes"))
    })
}

fn from_le_words(words: &[u64]) -> Hash {
    let mut out = [0u8; 32];
    for (chunk, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(words) {
        *chunk = word.to_le_bytes();
    }
    out
}

/// `cSHAKE256("ProofOfWorkHash")` with its one domain block already absorbed
/// and the padding for a 80-byte message pre-applied (upstream's comment:
/// `[10] ^= 0x04`, `[16] ^= 1 << 63`).
#[rustfmt::skip]
pub const POW_INITIAL_STATE: [u64; 25] = [
@POW_STATE@
];

/// `cSHAKE256("HeavyHash")`, likewise, for a 32-byte message.
#[rustfmt::skip]
pub const HEAVY_INITIAL_STATE: [u64; 25] = [
@HEAVY_STATE@
];

/// `PRE_POW_HASH ‖ TIME ‖ 32 zero bytes`, absorbed; the nonce is the last word.
#[derive(Clone)]
pub struct PowHasher([u64; 25]);

impl PowHasher {
    /// The sponge after `pre_pow_hash` and `timestamp`.
    #[must_use]
    pub fn new(pre_pow_hash: &Hash, timestamp: u64) -> Self {
        let mut state = POW_INITIAL_STATE;
        for (word, pre) in state.iter_mut().zip(le_words(pre_pow_hash)) {
            *word ^= pre;
        }
        state[4] ^= timestamp;
        Self(state)
    }

    /// `cSHAKE256("ProofOfWorkHash")` of the whole 80-byte message.
    #[must_use]
    pub fn finalize_with_nonce(&self, nonce: u64) -> Hash {
        let mut state = self.0;
        state[9] ^= nonce;
        keccak::f1600(&mut state);
        from_le_words(&state[..4])
    }
}

/// `cSHAKE256("HeavyHash")` of a 32-byte input.
#[must_use]
pub fn heavy_hash_outer(input: &Hash) -> Hash {
    let mut state = HEAVY_INITIAL_STATE;
    for (word, value) in state.iter_mut().zip(le_words(input)) {
        *word ^= value;
    }
    keccak::f1600(&mut state);
    from_le_words(&state[..4])
}

/// xoshiro256++, seeded from a hash's four little-endian words.
struct XoShiRo256PlusPlus([u64; 4]);

impl XoShiRo256PlusPlus {
    fn new(seed: &Hash) -> Self {
        Self(le_words(seed))
    }

    fn next(&mut self) -> u64 {
        let [s0, s1, s2, s3] = &mut self.0;
        let result = s0.wrapping_add(s0.wrapping_add(*s3).rotate_left(23));
        let t = *s1 << 17;
        *s2 ^= *s0;
        *s3 ^= *s1;
        *s1 ^= *s2;
        *s0 ^= *s3;
        *s2 ^= t;
        *s3 = s3.rotate_left(45);
        result
    }
}

/// The 64×64 matrix of 4-bit values a block template's pre-PoW hash selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matrix(pub [[u16; 64]; 64]);

impl Matrix {
    /// Draws matrices from xoshiro256++ seeded by `seed` until one has full
    /// rank, exactly as upstream does.
    #[must_use]
    pub fn generate(seed: &Hash) -> Self {
        let mut rng = XoShiRo256PlusPlus::new(seed);
        loop {
            let mut rows = [[0u16; 64]; 64];
            for row in &mut rows {
                let mut value = 0u64;
                for (j, cell) in row.iter_mut().enumerate() {
                    let shift = j % 16;
                    if shift == 0 {
                        value = rng.next();
                    }
                    *cell = ((value >> (4 * shift)) & 0x0F) as u16;
                }
            }
            let matrix = Self(rows);
            if matrix.rank() == 64 {
                return matrix;
            }
        }
    }

    /// Upstream's rank computation, float for float.
    #[must_use]
    pub fn rank(&self) -> usize {
        const EPS: f64 = 1e-9;
        let mut m: [[f64; 64]; 64] =
            core::array::from_fn(|i| core::array::from_fn(|j| f64::from(self.0[i][j])));
        let mut rank = 0;
        let mut selected = [false; 64];
        for i in 0..64 {
            let Some(j) = (0..64).find(|&j| !selected[j] && m[j][i].abs() > EPS) else {
                continue;
            };
            rank += 1;
            selected[j] = true;
            for p in (i + 1)..64 {
                m[j][p] /= m[j][i];
            }
            for k in 0..64 {
                if k != j && m[k][i].abs() > EPS {
                    for p in (i + 1)..64 {
                        m[k][p] -= m[j][p] * m[k][i];
                    }
                }
            }
        }
        rank
    }

    /// The per-nonce work: matrix × nibble vector, top four bits of each dot
    /// product, XOR with the input, then `cSHAKE256("HeavyHash")`.
    #[must_use]
    pub fn heavy_hash(&self, hash: &Hash) -> Hash {
        let mut nibbles = [0u16; 64];
        for (i, byte) in hash.iter().enumerate() {
            nibbles[2 * i] = u16::from(byte >> 4);
            nibbles[2 * i + 1] = u16::from(byte & 0x0F);
        }
        let mut product = [0u8; 32];
        for (i, out) in product.iter_mut().enumerate() {
            let dot = |row: &[u16; 64]| -> u16 {
                row.iter().zip(&nibbles).map(|(a, b)| a * b).sum()
            };
            // At most 64 × 15 × 15 = 14,400 < 2^14, so `>> 10` leaves four bits.
            let high = dot(&self.0[2 * i]) >> 10;
            let low = dot(&self.0[2 * i + 1]) >> 10;
            *out = u8::try_from((high << 4) | low).expect("two four-bit halves") ^ hash[i];
        }
        heavy_hash_outer(&product)
    }
}

/// A block template's mining state: the matrix and the absorbed header.
pub struct State {
    matrix: Matrix,
    hasher: PowHasher,
}

impl State {
    /// Built once per template.
    #[must_use]
    pub fn new(pre_pow_hash: &Hash, timestamp: u64) -> Self {
        Self {
            matrix: Matrix::generate(pre_pow_hash),
            hasher: PowHasher::new(pre_pow_hash, timestamp),
        }
    }

    /// The hash one nonce produces: what a miner computes per attempt.
    #[must_use]
    pub fn pow(&self, nonce: u64) -> Hash {
        self.matrix.heavy_hash(&self.hasher.finalize_with_nonce(nonce))
    }
}"#;

/// The known-answer fixture for the reference tests.
const FIXTURE_TEMPLATE: &str = r"//! Known answers for the kHeavyHash reference, copied from rusty-kaspa commit
//! `@COMMIT@`, `consensus/pow/src/matrix.rs` (`test_heavy_hash` and
//! `test_generate_matrix`), by `cargo xtask kheavyhash` — not retyped. ISC licence,
//! copyright the Kaspa developers.

/// `test_heavy_hash`: the input hash.
pub const HEAVY_HASH_INPUT: [u8; 32] = [@HH_INPUT@];

/// `test_heavy_hash`: the expected output.
pub const HEAVY_HASH_EXPECTED: [u8; 32] = [@EXPECTED@];

/// `test_heavy_hash`: the fixed matrix.
#[rustfmt::skip]
pub const HEAVY_HASH_MATRIX: [[u16; 64]; 64] = [
@HH_MATRIX@
];

/// `test_generate_matrix`: the seed.
pub const GENERATE_SEED: [u8; 32] = [42; 32];

/// `test_generate_matrix`: the matrix that seed must produce.
#[rustfmt::skip]
pub const GENERATED_MATRIX: [[u16; 64]; 64] = [
@GEN_MATRIX@
];";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn digit_runs_splits_on_anything_that_is_not_a_digit() {
        assert_eq!(digit_runs("a, 12,3 ;456"), vec!["12", "3", "456"]);
    }

    #[test]
    fn five_words_per_line_with_python_spacing() {
        let words: Vec<String> = (1..=7).map(|n| n.to_string()).collect();
        assert_eq!(fmt_words(&words), "    1, 2, 3, 4, 5,\n    6, 7,");
    }

    #[test]
    fn initial_state_is_read_from_the_named_impl_only() {
        let text = "impl PowHash { const INITIAL_STATE: [u64; 25] = [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25]; } impl KHeavyHash { INITIAL_STATE: [u64; 25] = [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]; }";
        let pow = initial_state(text, "PowHash").expect("25 words");
        assert_eq!(pow.first().map(String::as_str), Some("1"));
        assert_eq!(pow.last().map(String::as_str), Some("25"));
    }

    #[test]
    fn a_missing_marker_is_an_error_not_a_panic() {
        assert!(
            numbers_after(
                "no vectors here",
                "let expected_hash = Hash::from_bytes([",
                32
            )
            .is_err()
        );
    }

    #[test]
    fn the_templates_carry_every_placeholder_the_generator_fills() {
        for marker in ["@COMMIT@", "@POW_STATE@", "@HEAVY_STATE@"] {
            assert!(MODULE_TEMPLATE.contains(marker), "module lacks {marker}");
        }
        for marker in [
            "@COMMIT@",
            "@HH_INPUT@",
            "@EXPECTED@",
            "@HH_MATRIX@",
            "@GEN_MATRIX@",
        ] {
            assert!(FIXTURE_TEMPLATE.contains(marker), "fixture lacks {marker}");
        }
    }
}
