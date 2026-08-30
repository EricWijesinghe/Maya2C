//! Hybrid ML-DSA-65 + SLH-DSA-SHA2-128s transaction authorization.
//!
//! Every transaction on this chain carries two signatures over the same bytes:
//! a lattice proof under FIPS 204 and a hash-based proof under FIPS 205. Both
//! must verify. Neither is a fallback for the other.
//!
//! ## Why two
//!
//! ML-DSA-65 is already post-quantum, so a second post-quantum scheme looks
//! redundant. It is not, because the two are post-quantum for unrelated reasons.
//!
//! ML-DSA rests on Module-LWE — a structured lattice assumption about fifteen
//! years old, believed hard, not proven hard. The ring structure that makes it
//! fast is the same structure a future attack would attack. SLH-DSA rests on
//! nothing but the preimage and collision resistance of SHA-2: no algebra, no
//! structure, and a security argument that predates the chain by decades.
//!
//! A forgery must therefore break lattices *and* hashes. A cryptanalytic
//! advance against either one alone leaves the ledger intact, which is the only
//! reason the cost below is worth paying.
//!
//! ## What it costs
//!
//! Measured by `cargo bench --bench hybrid_signing`, over a real transaction's
//! signing bytes:
//!
//! | | ML-DSA-65 | SLH-DSA-SHA2-128s | hybrid |
//! |---|---|---|---|
//! | public key | 1952 B | 32 B | 1984 B |
//! | signature | 3309 B | 7856 B | 11165 B |
//! | keygen | 0.14 ms | 14 ms | 14 ms |
//! | sign | 0.22 ms | 105 ms | 105 ms |
//! | verify | 0.099 ms | 0.107 ms | 0.18 ms |
//!
//! Signing is roughly five hundred times more expensive than it was.
//! Verification is under twice. That asymmetry is the whole design: signing
//! happens once, in a wallet, per transaction; verification happens on every
//! node, for every transaction, in every block, forever.
//!
//! The verify column also explains the choice of the `s` parameter set over
//! `f`. Measured side by side, `f` signs twenty times faster but verifies at
//! 0.56 ms against 0.22 ms and carries a 17088-byte signature — it optimizes
//! the operation that happens once at the expense of the one that happens
//! forever. See `maya_crypto_pq` for that comparison.
//!
//! ## The address must commit to both keys
//!
//! This is the part that is easy to get wrong, and getting it wrong makes the
//! entire feature decorative.
//!
//! Suppose the address kept committing only to the ML-DSA key, as it did in v2.
//! An adversary who broke Module-LWE could forge the lattice half for any
//! account — and then simply *generate their own SLH-DSA keypair*, sign with
//! it, and attach it to the transaction. Both checks pass. The hash-based half
//! would have added 7856 bytes and zero security, because nothing pinned it to
//! the victim.
//!
//! So an address is `blake3(v3 ‖ ml_dsa_pk ‖ slh_dsa_pk)`, and
//! [`HybridPublicKey`] travels as a unit. Breaking one scheme now yields a
//! forged signature under a key that hashes to a *different* address, which
//! spends nothing.
//!
//! [`crate::core::Transaction::signing_bytes`] additionally commits to both
//! keys, so each signature covers the other's key: neither half can be lifted
//! out of one transaction and replayed beside a different partner key.
//!
//! ## Consequence: every address changed
//!
//! v2 addresses are hashes of an ML-DSA key alone and have no v3 preimage. This
//! is a hard fork with a new genesis, not a migration. There is deliberately no
//! compatibility path: accepting a v2 address would mean accepting a
//! single-signed transaction, which is the exact thing this module exists to
//! prevent.

use maya_crypto_pq::sig as slh;
use zeroize::Zeroizing;

use crate::core::codec::ByteReader;
use crate::crypto::keys::{
    self, PUBLIC_KEY_LEN as ML_DSA_PUBLIC_KEY_LEN, SECRET_KEY_LEN as ML_DSA_SECRET_KEY_LEN,
    SIGNATURE_LENGTH as ML_DSA_SIGNATURE_LENGTH,
};
use crate::error::{NodeError, Result};

/// Re-exported so callers reach one module for everything about an account.
pub use crate::crypto::keys::ADDRESS_LEN;

/// Length of an encoded SLH-DSA-SHA2-128s public key, in bytes.
pub const SLH_DSA_PUBLIC_KEY_LEN: usize = slh::PUBLIC_KEY_LEN;

/// Length of an encoded SLH-DSA-SHA2-128s signature, in bytes.
pub const SLH_DSA_SIGNATURE_LENGTH: usize = slh::SIGNATURE_LEN;

/// Length of an encoded SLH-DSA-SHA2-128s private key, in bytes.
pub const SLH_DSA_SECRET_KEY_LEN: usize = slh::SECRET_KEY_LEN;

/// Encoded size of a [`HybridPublicKey`]: the lattice key then the hash key.
pub const HYBRID_PUBLIC_KEY_LEN: usize = ML_DSA_PUBLIC_KEY_LEN + SLH_DSA_PUBLIC_KEY_LEN;

/// Encoded size of a [`HybridSignature`]: the lattice proof then the hash proof.
pub const HYBRID_SIGNATURE_LENGTH: usize = ML_DSA_SIGNATURE_LENGTH + SLH_DSA_SIGNATURE_LENGTH;

/// Encoded size of a [`HybridSecretKey`] pair, in bytes.
pub const HYBRID_SECRET_KEY_LEN: usize = ML_DSA_SECRET_KEY_LEN + SLH_DSA_SECRET_KEY_LEN;

/// Domain separator for address derivation.
///
/// v3, because the preimage changed shape: v2 hashed an ML-DSA key alone. A
/// domain that outlived the key material it was minted for is a subtle way to
/// let two eras of the chain share an address space they cannot both satisfy.
const ADDRESS_DOMAIN: &[u8] = b"custom-l1-node.address.v3";

/// Domain separators splitting one chain key into two independent scheme seeds.
///
/// Feeding the same bytes to both schemes would make the two halves share their
/// entropy — a single seed compromise would then yield both keys, and the
/// independence the hybrid is built on would be fictional.
const LATTICE_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.ml-dsa-65.v1";
const HASH_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.slh-dsa-sha2-128s.v1";

/// A compile-time check on the sizes the wire format hard-codes.
const _: () = {
    assert!(
        HYBRID_PUBLIC_KEY_LEN == 1984,
        "hybrid public key must be 1952 + 32 bytes"
    );
    assert!(
        HYBRID_SIGNATURE_LENGTH == 11165,
        "hybrid signature must be 3309 + 7856 bytes"
    );
    assert!(
        HYBRID_SECRET_KEY_LEN == 4096,
        "hybrid secret key must be 4032 + 64 bytes"
    );
};

// ---------------------------------------------------------------------------
// public key
// ---------------------------------------------------------------------------

/// The pair of public keys that together name an account.
///
/// Carried as a unit everywhere. Splitting it — storing one half, or letting a
/// caller supply the two independently — is what reintroduces the attack the
/// module docs describe, so there is no constructor that takes only one.
#[derive(Clone, PartialEq, Eq)]
pub struct HybridPublicKey {
    /// ML-DSA-65 (FIPS 204) verifying key.
    pub lattice: [u8; ML_DSA_PUBLIC_KEY_LEN],
    /// SLH-DSA-SHA2-128s (FIPS 205) verifying key.
    pub hash_based: [u8; SLH_DSA_PUBLIC_KEY_LEN],
}

impl core::fmt::Debug for HybridPublicKey {
    /// Renders the address rather than four kilobytes of hex.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "HybridPublicKey({})", hex::encode(self.address()))
    }
}

impl Default for HybridPublicKey {
    /// An all-zero placeholder for an unsigned transaction.
    ///
    /// This *does* decode. Every 1952-byte string unpacks to some ML-DSA
    /// coefficient vector and every 32-byte string is a valid SLH-DSA verifying
    /// key, so neither half has a decode step to fail — there is no such thing
    /// as a structurally invalid hybrid public key. What protects the unsigned
    /// case is [`Transaction::verify`](crate::core::Transaction::verify)
    /// returning [`NodeError::MissingSignature`] before it ever looks at the
    /// key, and, behind that, the fact that nobody holds a secret key for this
    /// one.
    fn default() -> Self {
        Self {
            lattice: [0u8; ML_DSA_PUBLIC_KEY_LEN],
            hash_based: [0u8; SLH_DSA_PUBLIC_KEY_LEN],
        }
    }
}

impl HybridPublicKey {
    /// The address this key pair controls.
    #[must_use]
    pub fn address(&self) -> [u8; ADDRESS_LEN] {
        address_of(self)
    }

    /// Appends the canonical encoding: lattice key, then hash key.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.lattice);
        buf.extend_from_slice(&self.hash_based);
    }

    /// Reads a key pair from `reader`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if fewer than [`HYBRID_PUBLIC_KEY_LEN`]
    /// bytes remain. Neither half is validated here — that is
    /// [`HybridVerifyingKey::from_public_key`]'s job — so a malformed key
    /// surfaces at verification rather than at decode, where a peer could use
    /// it to distinguish error paths.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            lattice: reader.read_array::<ML_DSA_PUBLIC_KEY_LEN>()?,
            hash_based: reader.read_array::<SLH_DSA_PUBLIC_KEY_LEN>()?,
        })
    }
}

/// Derives an address from a hybrid public key.
///
/// `blake3(v3 ‖ ml_dsa_pk ‖ slh_dsa_pk)`, truncated to the 32 bytes the rest of
/// the system expects. Committing to both halves is what makes the second
/// signature load-bearing rather than decorative — see the module docs.
#[must_use]
pub fn address_of(public_key: &HybridPublicKey) -> [u8; ADDRESS_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ADDRESS_DOMAIN);
    hasher.update(&public_key.lattice);
    hasher.update(&public_key.hash_based);
    *hasher.finalize().as_bytes()
}

// ---------------------------------------------------------------------------
// signature
// ---------------------------------------------------------------------------

/// A pair of signatures over one message.
///
/// Both are produced over identical bytes — see
/// [`crate::core::Transaction::signing_bytes`] — and both must verify.
#[derive(Clone, PartialEq, Eq)]
pub struct HybridSignature {
    /// ML-DSA-65 (FIPS 204) proof.
    pub lattice: [u8; ML_DSA_SIGNATURE_LENGTH],
    /// SLH-DSA-SHA2-128s (FIPS 205) proof.
    pub hash_based: [u8; SLH_DSA_SIGNATURE_LENGTH],
}

impl core::fmt::Debug for HybridSignature {
    /// Eleven kilobytes of hex helps nobody; the prefixes identify it.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "HybridSignature(ml-dsa {}…, slh-dsa {}…)",
            hex::encode(&self.lattice[..8]),
            hex::encode(&self.hash_based[..8])
        )
    }
}

impl HybridSignature {
    /// Appends the canonical encoding: lattice proof, then hash proof.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.lattice);
        buf.extend_from_slice(&self.hash_based);
    }

    /// Reads a signature pair from `reader`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if fewer than [`HYBRID_SIGNATURE_LENGTH`]
    /// bytes remain.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            lattice: reader.read_array::<ML_DSA_SIGNATURE_LENGTH>()?,
            hash_based: reader.read_array::<SLH_DSA_SIGNATURE_LENGTH>()?,
        })
    }
}

// ---------------------------------------------------------------------------
// verifying key
// ---------------------------------------------------------------------------

/// A decoded hybrid verifying key, ready to check signatures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HybridVerifyingKey {
    lattice: keys::VerifyingKey,
    hash_based: slh::VerifyingKey,
}

impl HybridVerifyingKey {
    /// Decodes both halves of `public_key`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::MalformedPublicKey`] if the lattice half is not a
    /// valid ML-DSA-65 encoding. The hash half cannot fail — every 32-byte
    /// string is a valid SLH-DSA verifying key.
    pub fn from_public_key(public_key: &HybridPublicKey) -> Result<Self> {
        Ok(Self {
            lattice: keys::VerifyingKey::from_bytes(&public_key.lattice)?,
            hash_based: slh::VerifyingKey::from_bytes(&public_key.hash_based),
        })
    }

    /// The encoded key pair.
    #[must_use]
    pub fn to_public_key(&self) -> HybridPublicKey {
        HybridPublicKey {
            lattice: self.lattice.to_bytes(),
            hash_based: self.hash_based.to_bytes(),
        }
    }

    /// The address this key pair maps to.
    #[must_use]
    pub fn address(&self) -> [u8; ADDRESS_LEN] {
        self.to_public_key().address()
    }

    /// Verifies both signatures over `message`.
    ///
    /// Both must pass, and the check short-circuits on the first failure. That
    /// is what bounds the cost of a garbage transaction: measured, a signature
    /// rejected at the lattice half costs 70 µs against 179 µs for a full
    /// verification, so a node spends one verification on junk rather than two.
    /// Since an attacker chooses the rate at which junk arrives, that factor is
    /// worth having.
    ///
    /// The lattice half goes first because it is marginally the cheaper of the
    /// two (0.099 ms against 0.107 ms) — near enough to a tie that the ordering
    /// is a tiebreak, not the point. The fail-fast is the point.
    ///
    /// Short-circuiting leaks nothing here: signatures and keys are public data,
    /// and the only secret in the neighbourhood is the one nobody has.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::SignatureVerification`] if the ML-DSA proof fails
    /// and [`NodeError::HashSignatureVerification`] if the SLH-DSA proof does.
    /// The two are distinguished because a chain where one scheme starts
    /// failing and the other does not is a chain whose operators need to know
    /// which one, immediately.
    pub fn verify(&self, message: &[u8], signature: &HybridSignature) -> Result<()> {
        self.lattice.verify(message, &signature.lattice)?;
        self.hash_based
            .verify(message, &signature.hash_based)
            .map_err(|_| NodeError::HashSignatureVerification)
    }
}

// ---------------------------------------------------------------------------
// signing key
// ---------------------------------------------------------------------------

/// Both secret keys for one account.
#[derive(Clone)]
pub struct HybridSigningKey {
    lattice: keys::SigningKey,
    hash_based: slh::SigningKey,
}

impl core::fmt::Debug for HybridSigningKey {
    /// Deliberately opaque. A `Debug` that printed key material would put it in
    /// every log line that ever formatted a surrounding struct.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("HybridSigningKey(<redacted>)")
    }
}

impl HybridSigningKey {
    /// Assembles a hybrid key from two already-constructed halves.
    ///
    /// For the keystore, which stores the two encodings side by side. Prefer
    /// [`signing_key_from_seed`] anywhere a chain key is available: it
    /// guarantees the halves were derived together and are therefore actually
    /// the pair some address names.
    #[must_use]
    pub fn from_halves(lattice: keys::SigningKey, hash_based: slh::SigningKey) -> Self {
        Self {
            lattice,
            hash_based,
        }
    }

    /// Decodes a hybrid signing key from its 4096-byte encoding.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::MalformedPublicKey`] if either half is not a valid
    /// encoding for its scheme.
    pub fn from_bytes(bytes: &[u8; HYBRID_SECRET_KEY_LEN]) -> Result<Self> {
        let mut lattice = Zeroizing::new([0u8; ML_DSA_SECRET_KEY_LEN]);
        lattice.copy_from_slice(&bytes[..ML_DSA_SECRET_KEY_LEN]);

        let mut hash_based = Zeroizing::new([0u8; SLH_DSA_SECRET_KEY_LEN]);
        hash_based.copy_from_slice(&bytes[ML_DSA_SECRET_KEY_LEN..]);

        Ok(Self {
            lattice: keys::SigningKey::from_bytes(&lattice)?,
            hash_based: slh::SigningKey::from_bytes(&hash_based)
                .map_err(|_| NodeError::MalformedPublicKey)?,
        })
    }

    /// Encodes both secret keys, lattice half first.
    ///
    /// Returns a [`Zeroizing`] buffer so the copy is wiped on drop. The caller
    /// still owns the problem of where it writes those bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Zeroizing<[u8; HYBRID_SECRET_KEY_LEN]> {
        let mut out = Zeroizing::new([0u8; HYBRID_SECRET_KEY_LEN]);
        out[..ML_DSA_SECRET_KEY_LEN].copy_from_slice(self.lattice.to_bytes().as_slice());
        out[ML_DSA_SECRET_KEY_LEN..].copy_from_slice(self.hash_based.to_bytes().as_slice());
        out
    }

    /// The matching verifying keys.
    #[must_use]
    pub fn verifying_key(&self) -> HybridVerifyingKey {
        HybridVerifyingKey {
            lattice: self.lattice.verifying_key(),
            hash_based: self.hash_based.verifying_key(),
        }
    }

    /// The encoded public key pair.
    #[must_use]
    pub fn public_key(&self) -> HybridPublicKey {
        self.verifying_key().to_public_key()
    }

    /// The address this key pair controls.
    #[must_use]
    pub fn address(&self) -> [u8; ADDRESS_LEN] {
        self.public_key().address()
    }

    /// Signs `message` under both schemes.
    ///
    /// Costs roughly 105 ms, essentially all of it in the hash-based half —
    /// the lattice signature is 0.2 % of the total. That is a wallet-side cost
    /// paid once per transaction; see the module docs for why it is the right
    /// side of the trade.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::SignatureVerification`] if the ML-DSA rejection
    /// loop fails to terminate, which FIPS 204 permits an implementation to
    /// report and which no caller can recover from. The hash-based half has no
    /// rejection loop and cannot fail.
    pub fn sign(&self, message: &[u8]) -> Result<HybridSignature> {
        Ok(HybridSignature {
            lattice: self.lattice.sign(message)?,
            hash_based: self.hash_based.sign(message),
        })
    }
}

/// Derives a hybrid signing key deterministically from a 32-byte chain key.
///
/// The wallet derives account keys through SLIP-0010, which produces one
/// 32-byte hardened chain key per path. That single value is split here into
/// two domain-separated seeds, one per scheme, so a mnemonic still yields one
/// reproducible set of accounts while the two halves remain independent.
///
/// As with the ML-DSA-only construction it replaces, this derivation is
/// **specific to this chain**. SLIP-0010 covers ed25519 and secp256k1; there is
/// no standardized hierarchical derivation for either ML-DSA or SLH-DSA, so no
/// other wallet will recover these accounts from the same mnemonic.
///
/// # Errors
///
/// Returns [`NodeError::Network`] if ML-DSA key generation fails, which for a
/// fixed seed indicates a defect rather than a transient condition.
pub fn signing_key_from_seed(chain_key: &[u8; 32]) -> Result<HybridSigningKey> {
    let lattice_seed = derive_seed(LATTICE_SEED_DOMAIN, chain_key);
    let hash_seed = derive_seed(HASH_SEED_DOMAIN, chain_key);

    Ok(HybridSigningKey {
        lattice: keys::signing_key_from_seed(&lattice_seed)?,
        hash_based: slh::signing_key_from_seed(&hash_seed),
    })
}

/// Generates a fresh hybrid signing key from the operating system CSPRNG.
///
/// Drawn as one 32-byte chain key and expanded, rather than generating the two
/// halves independently, so a generated key and a derived key travel exactly
/// the same code path. One path means one thing to audit.
///
/// # Errors
///
/// Returns [`NodeError::Network`] if the OS entropy source is unavailable.
/// Callers must treat this as fatal rather than retrying with a fallback:
/// silently producing a key from degraded randomness is the one outcome a
/// signing key must never have.
pub fn generate_signing_key() -> Result<HybridSigningKey> {
    let mut chain_key = Zeroizing::new([0u8; 32]);
    getrandom::fill(chain_key.as_mut_slice())
        .map_err(|e| NodeError::Network(format!("OS entropy source unavailable: {e}")))?;
    signing_key_from_seed(&chain_key)
}

/// Splits one chain key into a scheme-specific seed.
fn derive_seed(domain: &[u8], chain_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(chain_key);
    Zeroizing::new(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> HybridSigningKey {
        signing_key_from_seed(&[seed; 32]).expect("derive")
    }

    #[test]
    fn the_wire_sizes_are_what_the_encodings_assume() {
        assert_eq!(HYBRID_PUBLIC_KEY_LEN, 1984);
        assert_eq!(HYBRID_SIGNATURE_LENGTH, 11165);
        assert_eq!(HYBRID_SECRET_KEY_LEN, 4096);
    }

    #[test]
    fn both_signatures_verify_under_their_own_key() {
        let signer = key(1);
        let signature = signer.sign(b"maya2c").expect("sign");
        assert_eq!(signer.verifying_key().verify(b"maya2c", &signature), Ok(()));
    }

    #[test]
    fn signing_is_deterministic_in_both_halves() {
        // Load-bearing for consensus: txid hashes both signatures, so a hedged
        // signer in either half would give one payload two different txids.
        let signer = key(2);
        let first = signer.sign(b"maya2c").expect("sign");
        let second = signer.sign(b"maya2c").expect("sign");
        assert_eq!(first.lattice, second.lattice);
        assert_eq!(first.hash_based, second.hash_based);
    }

    #[test]
    fn a_forged_lattice_half_is_rejected() {
        let signer = key(3);
        let mut signature = signer.sign(b"maya2c").expect("sign");
        signature.lattice[0] ^= 0x01;
        assert_eq!(
            signer.verifying_key().verify(b"maya2c", &signature),
            Err(NodeError::SignatureVerification)
        );
    }

    #[test]
    fn a_forged_hash_half_is_rejected() {
        // The point of the whole module: a valid ML-DSA proof is not enough.
        let signer = key(4);
        let mut signature = signer.sign(b"maya2c").expect("sign");
        signature.hash_based[0] ^= 0x01;
        assert_eq!(
            signer.verifying_key().verify(b"maya2c", &signature),
            Err(NodeError::HashSignatureVerification)
        );
    }

    #[test]
    fn a_signature_half_cannot_be_paired_with_a_foreign_key() {
        // Models the attack the v3 address exists to stop: an adversary who
        // could forge the lattice half would still have to produce a hash-based
        // proof under the *victim's* SLH-DSA key, because the address commits
        // to both. Here the mismatch is simulated by swapping halves.
        let victim = key(5);
        let attacker = key(6);

        let forged = HybridSignature {
            lattice: victim.sign(b"steal").expect("sign").lattice,
            hash_based: attacker.sign(b"steal").expect("sign").hash_based,
        };

        assert_eq!(
            victim.verifying_key().verify(b"steal", &forged),
            Err(NodeError::HashSignatureVerification)
        );
    }

    #[test]
    fn the_address_commits_to_both_keys() {
        // If it committed only to the lattice half, swapping the hash key would
        // leave the address unchanged and the second signature would be
        // unpinned — the exact hole described in the module docs.
        let base = key(7).public_key();

        let mut swapped_hash = base.clone();
        swapped_hash.hash_based = key(8).public_key().hash_based;
        assert_ne!(base.address(), swapped_hash.address());

        let mut swapped_lattice = base.clone();
        swapped_lattice.lattice = key(8).public_key().lattice;
        assert_ne!(base.address(), swapped_lattice.address());
    }

    #[test]
    fn address_derivation_is_domain_separated() {
        let public_key = key(9).public_key();
        let mut undomained = blake3::Hasher::new();
        undomained.update(&public_key.lattice);
        undomained.update(&public_key.hash_based);
        assert_ne!(public_key.address(), *undomained.finalize().as_bytes());
    }

    #[test]
    fn the_v3_address_differs_from_the_v2_address_of_the_same_lattice_key() {
        // The hard fork, asserted. A v2 address was blake3 over the ML-DSA key
        // alone; nothing may make the two spaces coincide.
        let signer = key(10);
        let public_key = signer.public_key();
        let v2 = keys::address_of_lattice_only(&public_key.lattice);
        assert_ne!(public_key.address(), v2);
    }

    #[test]
    fn the_two_scheme_seeds_are_independent() {
        // Same chain key, different domains: the derived seeds must not match,
        // or the halves would share their entropy.
        let chain_key = [11u8; 32];
        assert_ne!(
            *derive_seed(LATTICE_SEED_DOMAIN, &chain_key),
            *derive_seed(HASH_SEED_DOMAIN, &chain_key)
        );
    }

    #[test]
    fn keys_round_trip_through_their_encodings() {
        let signer = key(12);
        let signature = signer.sign(b"maya2c").expect("sign");

        let restored = HybridSigningKey::from_bytes(&signer.to_bytes()).expect("decode");
        assert_eq!(restored.sign(b"maya2c").expect("re-sign"), signature);
        assert_eq!(restored.address(), signer.address());

        let public = HybridVerifyingKey::from_public_key(&signer.public_key()).expect("decode");
        assert_eq!(public.verify(b"maya2c", &signature), Ok(()));
        assert_eq!(public.address(), signer.address());
    }

    #[test]
    fn a_public_key_round_trips_through_the_wire_encoding() {
        let public_key = key(13).public_key();
        let mut buf = Vec::new();
        public_key.encode_into(&mut buf);
        assert_eq!(buf.len(), HYBRID_PUBLIC_KEY_LEN);

        let mut reader = ByteReader::new(&buf);
        let decoded = HybridPublicKey::decode(&mut reader).expect("decode");
        reader.finish().expect("consumed");
        assert_eq!(decoded, public_key);
    }

    #[test]
    fn a_signature_round_trips_through_the_wire_encoding() {
        let signature = key(14).sign(b"maya2c").expect("sign");
        let mut buf = Vec::new();
        signature.encode_into(&mut buf);
        assert_eq!(buf.len(), HYBRID_SIGNATURE_LENGTH);

        let mut reader = ByteReader::new(&buf);
        let decoded = HybridSignature::decode(&mut reader).expect("decode");
        reader.finish().expect("consumed");
        assert_eq!(decoded, signature);
    }

    #[test]
    fn a_seed_always_derives_the_same_account() {
        let first = key(15);
        let second = key(15);
        assert_eq!(first.address(), second.address());
    }

    #[test]
    fn different_seeds_derive_different_accounts() {
        assert_ne!(key(16).address(), key(17).address());
    }

    #[test]
    fn the_all_zero_placeholder_key_verifies_nothing() {
        // It decodes — neither scheme has a structurally invalid public key, so
        // there is no decode step to fail. The property that matters is the
        // outcome: no signature validates under it.
        let placeholder =
            HybridVerifyingKey::from_public_key(&HybridPublicKey::default()).expect("decodes");
        let signature = key(24).sign(b"maya2c").expect("sign");

        assert_eq!(
            placeholder.verify(b"maya2c", &signature),
            Err(NodeError::SignatureVerification)
        );
    }

    #[test]
    fn a_signing_key_does_not_print_its_material() {
        assert_eq!(format!("{:?}", key(18)), "HybridSigningKey(<redacted>)");
    }
}
