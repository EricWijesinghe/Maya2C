//! The structured reference string, and why this one must never secure value.
//!
//! # KZG needs a secret nobody knows. This one is known to everybody
//!
//! A KZG setup is the powers `τ^i·G` of a secret `τ`. Whoever knows `τ` can
//! open a commitment to any value they like, which means they can prove any
//! model said anything. A real deployment uses a ceremony where `τ` was the
//! product of many contributions and is secure if *one* contributor destroyed
//! theirs.
//!
//! [`params`] is not that. It derives `τ` from [`TESTNET_SRS_SEED`] — a string
//! in this file — so `τ` is public and **anyone who reads this file can forge
//! a zkML proof for any model and any output**. That is the correct setup for
//! tests and for a testnet whose proofs secure nothing, and it is disqualifying
//! for anything else.
//!
//! So [`SRS_IS_TRUSTED`] is `false`, [`check_chain`] refuses a value-bearing
//! chain id, and the host function is behind an activation height of
//! `u64::MAX`. That is the same arrangement `zk-privacy` has for its Groth16
//! parameters (`SETUP_IS_TRUSTED`), for the same reason.
//!
//! # What replacing it takes
//!
//! A published universal ceremony — the perpetual powers of tau used across the
//! halo2 ecosystem — converted to `ParamsKZG` at `K`, loaded here in place of
//! the seeded setup, with its hash pinned in a test. `SRS_IS_TRUSTED` flips
//! only when that is done, and flipping it is a decision somebody writes down.

use std::sync::OnceLock;

use halo2_axiom::halo2curves::bn256::Bn256;
use halo2_axiom::poly::kzg::commitment::ParamsKZG;

use crate::error::{Result, ZkmlError};

/// Circuit size: `2^K` rows.
///
/// Every verifying key the verifier accepts must declare exactly this, because
/// the SRS is sized to it. Fixing it is also what stops a hostile key from
/// asking the verifier to build a `2^30`-point evaluation domain.
pub const K: u32 = 11;

/// Whether the SRS below may secure value. It may not.
pub const SRS_IS_TRUSTED: bool = false;

/// The seed `τ` is derived from. Public, which is the whole problem.
pub const TESTNET_SRS_SEED: &[u8] = b"maya2c.zkml.testnet-srs.v1.the-toxic-waste-is-public";

/// Chain ids that hold value.
///
/// Duplicated rather than imported, like every other guard in this workspace
/// (`apps/faucet/src/lib.rs:50`, `bins/maya2c-node/src/main.rs:91`): the guard must not depend on
/// the crate it guards against.
const VALUE_BEARING_CHAINS: &[&str] = &["maya-mainnet", "mainnet"];

/// Refuses to let this SRS near a chain that holds value.
///
/// # Errors
///
/// [`ZkmlError::UntrustedSetup`] for a value-bearing chain while
/// [`SRS_IS_TRUSTED`] is `false`.
pub fn check_chain(chain_id: &str) -> Result<()> {
    if !SRS_IS_TRUSTED && VALUE_BEARING_CHAINS.contains(&chain_id) {
        return Err(ZkmlError::UntrustedSetup);
    }
    Ok(())
}

/// The KZG parameters, derived once per process.
///
/// Cached because deriving them is `2^K` curve multiplications and they are
/// public — the reason `custody-mpc` avoids a lazy global (it would hold key
/// material) does not apply to data that is printed in this file's doc comment.
#[must_use]
pub fn params() -> &'static ParamsKZG<Bn256> {
    static PARAMS: OnceLock<ParamsKZG<Bn256>> = OnceLock::new();
    PARAMS.get_or_init(|| ParamsKZG::setup(K, SeedRng::new()))
}

/// A deterministic byte stream from the public seed.
///
/// BLAKE3's XOF rather than a seeded ChaCha: it is already a dependency, and
/// `ParamsKZG::setup` only needs `RngCore`, not a particular generator.
struct SeedRng(blake3::OutputReader);

impl SeedRng {
    fn new() -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(TESTNET_SRS_SEED);
        Self(hasher.finalize_xof())
    }
}

impl rand_core::RngCore for SeedRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        self.0.fill(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.0.fill(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill(dest);
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> core::result::Result<(), rand_core::Error> {
        self.0.fill(dest);
        Ok(())
    }
}
