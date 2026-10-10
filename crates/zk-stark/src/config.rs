//! The one STARK configuration — ADR-008.
//!
//! - **PCS:** FRI over `BabyBear` with the *hiding* variant, so proofs reveal
//!   nothing about the witness beyond the public values.
//! - **Commitments and transcript:** `Keccak-f[1600]`. The outer layer of the
//!   proof rests on the hash with the longest cryptanalysis record; Poseidon2
//!   is used only *inside* the AIRs, where it is cheap to arithmetise.
//! - **FRI:** blowup 4 (`log_blowup = 2`), 100 queries, 16 bits of query
//!   grinding — Plonky3's `new_benchmark_zk` preset. The security level this
//!   gives is *computed*, not asserted: [`crate::security_bits`] runs
//!   Plonky3's own estimator, and `reports/02-crypto.md` records both the
//!   conjectured and the proven figure.
//! - **Blinding randomness:** fresh from the OS for every proving
//!   configuration. A fixed seed would make proofs deterministic in the
//!   witness, which is exactly what zero-knowledge must not be.

use p3_challenger::{HashChallenger, SerializingChallenger32};
use p3_commit::ExtensionMmcs;
use p3_dft::Radix2DitParallel;
use p3_field::extension::BinomialExtensionField;
use p3_fri::{FriParameters, HidingFriPcs};
use p3_keccak::{Keccak256Hash, KeccakF, VECTOR_LEN};
use p3_merkle_tree::MerkleTreeHidingMmcs;
use p3_symmetric::{CompressionFunctionFromHasher, PaddingFreeSponge, SerializingHasher};
use p3_uni_stark::StarkConfig;
use rand::SeedableRng as _;
use rand::rngs::StdRng;

use crate::ZkError;
use crate::hash::F;

/// The degree-4 extension challenges are drawn from (~124 bits).
pub type Challenge = BinomialExtensionField<F, 4>;

type ByteHash = Keccak256Hash;
type U64Hash = PaddingFreeSponge<KeccakF, 25, 17, 4>;
type FieldHash = SerializingHasher<U64Hash>;
type Compress = CompressionFunctionFromHasher<U64Hash, 2, 4>;
type ValMmcs =
    MerkleTreeHidingMmcs<[F; VECTOR_LEN], [u64; VECTOR_LEN], FieldHash, Compress, StdRng, 2, 4, 4>;
type ChallengeMmcs = ExtensionMmcs<F, Challenge, ValMmcs>;
type Challenger = SerializingChallenger32<F, HashChallenger<u8, ByteHash, 32>>;
type Dft = Radix2DitParallel<F>;
type Pcs = HidingFriPcs<F, Dft, ValMmcs, ChallengeMmcs, StdRng>;

/// The STARK configuration type every prover and verifier here uses.
pub type Config = StarkConfig<Pcs, Challenge, Challenger>;

/// `log2` of the FRI blowup factor.
pub const LOG_BLOWUP: usize = 2;
/// FRI queries.
pub const NUM_QUERIES: usize = 100;
/// Grinding bits before the queries.
pub const QUERY_POW_BITS: usize = 16;
/// Random codewords the hiding PCS mixes in per committed matrix.
const NUM_RANDOM_CODEWORDS: usize = 4;

fn os_rng() -> Result<StdRng, ZkError> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|_| ZkError::Entropy)?;
    let rng = StdRng::from_seed(seed);
    zeroize::Zeroize::zeroize(&mut seed);
    Ok(rng)
}

/// A configuration with fresh blinding randomness. Use one per proof.
///
/// # Errors
///
/// [`ZkError::Entropy`] if the OS generator fails.
pub fn config() -> Result<Config, ZkError> {
    let u64_hash = U64Hash::new(KeccakF {});
    let val_mmcs = ValMmcs::new(
        FieldHash::new(u64_hash),
        Compress::new(u64_hash),
        0,
        os_rng()?,
    );
    let challenge_mmcs = ChallengeMmcs::new(val_mmcs.clone());
    let fri = FriParameters {
        log_blowup: LOG_BLOWUP,
        log_final_poly_len: 0,
        max_log_arity: 1,
        num_queries: NUM_QUERIES,
        commit_proof_of_work_bits: 0,
        query_proof_of_work_bits: QUERY_POW_BITS,
        mmcs: challenge_mmcs,
    };
    let pcs = Pcs::new(
        Dft::default(),
        val_mmcs,
        fri,
        NUM_RANDOM_CODEWORDS,
        os_rng()?,
    );
    Ok(Config::new(
        pcs,
        Challenger::from_hasher(vec![], ByteHash {}),
    ))
}
