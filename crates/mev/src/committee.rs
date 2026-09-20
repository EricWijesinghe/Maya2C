//! Who can decrypt, and how many of them it takes.

use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use zeroize::Zeroize;

use crate::error::{MevError, Result};
use crate::shamir::{self, SecretShare};

/// Bytes in a compressed ristretto255 point or a canonical scalar.
pub const ELEMENT_LEN: usize = 32;

/// The committee's public key, and the terms on which it decrypts.
///
/// `encryption_key` is `s·G` for a secret `s` that — after
/// [`Committee::generate`] returns — exists nowhere: it was split into shares
/// and the original dropped. Anyone can encrypt to it; `threshold` members
/// must cooperate to open the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committee {
    /// The aggregate encryption key.
    pub encryption_key: CompressedRistretto,
    /// Per-member verification keys, `s_i·G`, indexed by member index.
    ///
    /// Present so a decryption share can be *checked* rather than trusted. A
    /// committee without these can still decrypt, but it cannot tell a
    /// malformed share from a correct one until the plaintext comes out
    /// garbage — by which point the evidence of who caused it is gone.
    pub verification_keys: Vec<MemberKey>,
    /// Shares required to decrypt.
    pub threshold: u16,
}

/// One member's index and public verification key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberKey {
    /// The member's evaluation point, in `1..=members`.
    pub index: u16,
    /// `s_i·G`.
    pub key: CompressedRistretto,
}

/// A member's private share of the committee key.
///
/// Zeroized on drop, and deliberately not `Clone`: a share that can be copied
/// freely is a share that ends up in a `Vec` somewhere nobody is thinking about.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct MemberSecret {
    /// The member's evaluation point.
    pub index: u16,
    /// `f(index)` — this member's piece of the committee secret.
    pub(crate) scalar: Scalar,
}

impl core::fmt::Debug for MemberSecret {
    /// Index only. See [`crate::shamir::SecretShare`] for why.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MemberSecret")
            .field("index", &self.index)
            .field("scalar", &"<redacted>")
            .finish()
    }
}

impl MemberSecret {
    /// This member's public verification key.
    #[must_use]
    pub fn verification_key(&self) -> CompressedRistretto {
        RistrettoPoint::mul_base(&self.scalar).compress()
    }
}

impl Committee {
    /// Generates a fresh committee key and splits it into member secrets.
    ///
    /// # This is a trusted setup, and it is the weakest part of the scheme
    ///
    /// Whoever runs this function holds the whole committee secret for the
    /// duration of the call. A distributed key generation protocol — each
    /// member contributing a polynomial, nobody ever holding `s` — removes
    /// that, and **is not built here**. The intended deployment is generation
    /// at genesis alongside the chain's other parameters, exactly as
    /// `zk-privacy` handles its Groth16 setup, with the same honest caveat:
    /// a compromised setup means every sealed transaction was readable from
    /// the start.
    ///
    /// What that costs is bounded, and worth stating precisely. A leaked setup
    /// does **not** let anyone forge a transaction, spend a coin, or change a
    /// balance — sealed transactions are still signed by their senders and
    /// still validated in full once opened. It costs *confidentiality before
    /// ordering*, which is to say it costs exactly the MEV protection and
    /// nothing else. The chain degrades to a plaintext mempool, which is what
    /// every chain without this feature already is.
    ///
    /// # Errors
    ///
    /// - [`MevError::EmptyCommittee`] for `members == 0`.
    /// - [`MevError::InvalidThreshold`] unless `1 <= threshold <= members`.
    /// - [`MevError::EntropyFailure`] if the OS entropy source is unavailable.
    pub fn generate(threshold: u16, members: u16) -> Result<(Self, Vec<MemberSecret>)> {
        let mut secret = shamir::random_scalar()?;
        let encryption_key = RistrettoPoint::mul_base(&secret).compress();

        let shares = shamir::split(&secret, threshold, members)?;
        // The only copy of the committee secret held by this process, gone
        // before the function returns. Everything after this point works from
        // shares.
        secret.zeroize();

        let secrets: Vec<MemberSecret> = shares
            .iter()
            .map(|SecretShare { index, value }| MemberSecret {
                index: *index,
                scalar: *value,
            })
            .collect();

        let verification_keys = secrets
            .iter()
            .map(|member| MemberKey {
                index: member.index,
                key: member.verification_key(),
            })
            .collect();

        Ok((
            Self {
                encryption_key,
                verification_keys,
                threshold,
            },
            secrets,
        ))
    }

    /// Members holding a share.
    #[must_use]
    pub fn members(&self) -> u16 {
        // A committee is built by `generate`, which caps members at `u16`.
        u16::try_from(self.verification_keys.len()).unwrap_or(u16::MAX)
    }

    /// How many members may be unavailable before decryption stalls.
    ///
    /// The liveness budget, stated as a number rather than left implicit. With
    /// `t` of `n`, decryption survives `n − t` absentees and stops at `n − t + 1`
    /// — and "stops" means sealed transactions expire unopened, never that they
    /// execute in the clear. See [the crate documentation](crate) on why that direction is the
    /// safe one.
    #[must_use]
    pub fn absentee_budget(&self) -> u16 {
        self.members().saturating_sub(self.threshold)
    }

    /// The decompressed encryption key.
    ///
    /// # Errors
    ///
    /// [`MevError::MalformedPoint`] if the stored bytes are not a canonical
    /// ristretto255 encoding — reachable only for a committee decoded from
    /// untrusted bytes, which is why it is checked rather than asserted.
    pub fn point(&self) -> Result<RistrettoPoint> {
        self.encryption_key
            .decompress()
            .ok_or(MevError::MalformedPoint)
    }

    /// The verification key registered for `index`.
    #[must_use]
    pub fn verification_key(&self, index: u16) -> Option<CompressedRistretto> {
        self.verification_keys
            .iter()
            .find(|member| member.index == index)
            .map(|member| member.key)
    }
}
