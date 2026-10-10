//! `ArgonBlake`: a memory-hard hybrid proof-of-work hash.
//!
//! Three stages:
//!
//! 1. **BLAKE3 pre-hash** of the raw header bytes, compressing arbitrary-length
//!    input to a fixed 32-byte block that becomes the Argon2 "password".
//! 2. **Argon2id** over 32 MiB, which supplies the memory hardness — the reason
//!    this construction resists ASIC and GPU acceleration.
//! 3. **BLAKE3 squeeze** (XOF) over the Argon2 output, producing the final
//!    digest.
//!
//! ## Performance, and why there is no SIMD tuning here
//!
//! Measured on this codebase with `cargo bench --bench argon_blake`:
//!
//! | Stage | Time |
//! |---|---|
//! | Full `ArgonBlake` hash | ~25.4 ms |
//! | BLAKE3 pre-hash alone | ~120 ns |
//!
//! The BLAKE3 stages are **0.0005%** of the total. Argon2id over 32 MiB is
//! essentially the entire cost, by design — that memory-hardness is the point
//! of the construction.
//!
//! Three facts follow, and together they close off SIMD tuning as an option:
//!
//! - `blake3` is already SIMD-accelerated with runtime CPU dispatch. The crate
//!   exposes only `no_avx2` / `no_avx512` / `no_sse41` *opt-outs*; there is
//!   nothing to enable.
//! - `argon2 0.5` ships no SIMD feature at all. It is a portable scalar
//!   implementation.
//! - Building with `-C target-feature=+avx2` was measured at ~27.0 ms against a
//!   ~25.4 ms baseline: no improvement, well inside run-to-run noise.
//!
//! So no target-feature flags are set. Beyond being useless here, `target-cpu`
//! tuning in the release image would risk `SIGILL` on any deployment host older
//! than the build host.
//!
//! The only remaining lever would be replacing the Argon2 implementation, which
//! would change the digest. That is a hard fork: see
//! `argon_blake_matches_the_frozen_known_answer` in `tests/crypto_tests.rs`.
//!
//! ## Why the salt is derived, not random
//!
//! Proof-of-work must be *verifiable*: any node holding only the header bytes
//! has to recompute an identical digest. A random salt would make that
//! impossible, so the salt is derived deterministically from the header under
//! its own domain-separation context. Distinct headers still get distinct
//! salts, so Argon2's memory cost cannot be amortized across candidates.

use argon2::{Algorithm, Argon2, Params, Version};

use crate::error::{NodeError, Result};

/// Length of an `ArgonBlake` digest, in bytes.
pub const HASH_LEN: usize = 32;

/// Argon2 memory cost in KiB. 32 MiB, as specified by the consensus rules.
pub const ARGON_MEMORY_KIB: u32 = 32 * 1024;

/// Argon2 time cost (number of passes).
pub const ARGON_TIME_COST: u32 = 1;

/// Argon2 parallelism (lanes). Fixed at 1 so the digest is independent of the
/// verifier's core count.
pub const ARGON_LANES: u32 = 1;

/// Salt length in bytes. Argon2 requires at least 8.
const SALT_LEN: usize = 16;

// Domain-separation contexts. Distinct constants keep the three BLAKE3 uses
// from ever colliding, even on identical input.
const SALT_CONTEXT: &str = "custom-l1-node 2026-08-27 argonblake salt v1";
const SQUEEZE_CONTEXT: &str = "custom-l1-node 2026-08-27 argonblake squeeze v1";

/// Computes the `ArgonBlake` digest of `header_bytes`.
///
/// Deterministic: equal input always yields an equal digest, which is what
/// makes the proof-of-work verifiable by any node.
///
/// # Errors
///
/// Returns [`NodeError::InvalidArgonParams`] if the compile-time parameter set
/// is rejected, or [`NodeError::ArgonHash`] if the Argon2id pass fails (for
/// example, if the 32 MiB working buffer cannot be allocated).
pub fn argon_blake_hash(header_bytes: &[u8]) -> Result<[u8; HASH_LEN]> {
    // Stage 1: BLAKE3 pre-hash. Compresses variable-length header bytes into
    // the fixed-size input Argon2 expects.
    let prehash = blake3::hash(header_bytes);
    let prehash_bytes = prehash.as_bytes();

    // Deterministic, header-bound salt under its own context.
    let salt_material = blake3::derive_key(SALT_CONTEXT, header_bytes);
    let salt = &salt_material[..SALT_LEN];

    // Stage 2: Argon2id over 32 MiB.
    let params = Params::new(
        ARGON_MEMORY_KIB,
        ARGON_TIME_COST,
        ARGON_LANES,
        Some(HASH_LEN),
    )
    .map_err(|e| NodeError::InvalidArgonParams(e.to_string()))?;

    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut argon_out = [0u8; HASH_LEN];
    argon
        .hash_password_into(prehash_bytes, salt, &mut argon_out)
        .map_err(|e| NodeError::ArgonHash(e.to_string()))?;

    // Stage 3: BLAKE3 squeeze. The pre-hash is folded back in so the final
    // digest commits to the original header and not only to Argon2's output.
    let mut hasher = blake3::Hasher::new_derive_key(SQUEEZE_CONTEXT);
    hasher.update(&argon_out);
    hasher.update(prehash_bytes);

    let mut digest = [0u8; HASH_LEN];
    hasher.finalize_xof().fill(&mut digest);
    Ok(digest)
}
