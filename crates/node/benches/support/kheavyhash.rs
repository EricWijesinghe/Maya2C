//! kHeavyHash — Kaspa's proof of work — as a **reference implementation for
//! comparison benchmarks only**. Nothing in the node links it, and it is not a
//! consensus rule here: this chain's own proof of work is `argon_blake`.
//!
//! # Where it comes from, and how it is checked
//!
//! Ported from rusty-kaspa (ISC licence, copyright the Kaspa developers) at
//! commit `01b532e8b553523216471682649693af92f0fd16`: `consensus/pow/src/{matrix,xoshiro}.rs` and
//! `crypto/hashes/src/pow_hashers.rs`. The two sponge initial states below are
//! copied from upstream by `gen_kheavyhash.py`, not retyped.
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
    1242148031264380989, 3008272977830772284, 2188519011337848018, 1992179434288343456, 8876506674959887717,
    5399642050693751366, 1745875063082670864, 8605242046444978844, 17936695144567157056, 3343109343542796272,
    1123092876221303306, 4963925045340115282, 17037383077651887893, 16629644495023626889, 12833675776649114147,
    3784524041015224902, 1082795874807940378, 13952716920571277634, 13411128033953605860, 15060696040649351053,
    9928834659948351306, 5237849264682708699, 12825353012139217522, 6706187291358897596, 196324915476054915,
];

/// `cSHAKE256("HeavyHash")`, likewise, for a 32-byte message.
#[rustfmt::skip]
pub const HEAVY_INITIAL_STATE: [u64; 25] = [
    4239941492252378377, 8746723911537738262, 8796936657246353646, 1272090201925444760, 16654558671554924250,
    8270816933120786537, 13907396207649043898, 6782861118970774626, 9239690602118867528, 11582319943599406348,
    17596056728278508070, 15212962468105129023, 7812475424661425213, 3370482334374859748, 5690099369266491460,
    8596393687355028144, 570094237299545110, 9119540418498120711, 16901969272480492857, 13372017233735502424,
    14372891883993151831, 5171152063242093102, 10573107899694386186, 6096431547456407061, 1592359455985097269,
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
            let dot =
                |row: &[u16; 64]| -> u16 { row.iter().zip(&nibbles).map(|(a, b)| a * b).sum() };
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
        self.matrix
            .heavy_hash(&self.hasher.finalize_with_nonce(nonce))
    }
}
