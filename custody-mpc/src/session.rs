//! The quorum signing session — and the honest account of what it is.
//!
//! # This is threshold custody, not a threshold signature scheme
//!
//! Say it before anything else, because the difference decides where an
//! institution puts its trust.
//!
//! A threshold signature scheme never assembles the key. Custodians compute
//! *partial signatures* from their shares and someone adds them up; no machine
//! ever holds the signing key, not even for a microsecond. That is what MPC-TSS
//! means, and it is what FROST does for Ed25519.
//!
//! What happens here is different. A quorum's shares are sent to one combiner,
//! which interpolates the vault secret, derives the hybrid key, signs, and
//! drops everything. For the length of one signature, one machine can sign
//! anything the vault owns.
//!
//! # Why, and it is not laziness
//!
//! A Maya2C signature is a hybrid pair: ML-DSA-65 (FIPS 204) **and**
//! SLH-DSA-SHA2-128s (FIPS 205). `HybridVerifyingKey::verify`
//! (`src/crypto/hybrid.rs:337`) checks both, so a scheme that thresholds one
//! half produces nothing the chain accepts.
//!
//! - **The lattice half.** Threshold ML-DSA was an open problem until 2025 and
//!   is now the subject of several competing constructions, none standardised
//!   and none with a production implementation. `fips204` — the only pure-Rust
//!   ML-DSA in this workspace, and the one consensus is pinned to — exposes
//!   `try_keygen`, `keygen_from_seed`, `try_sign` and `try_sign_with_seed`, and
//!   nothing that decomposes into partial signatures.
//! - **The hash-based half.** There is no threshold SLH-DSA. Not "not
//!   standardised" — no construction. A hash-based signature is a walk down a
//!   Merkle structure with a secret seed, and thresholding it means running the
//!   whole hash tree inside generic MPC.
//!
//! `docs/custody-mpc.md` has the citations and the full argument.
//!
//! # What this does buy
//!
//! | Property | Here | A real TSS |
//! |---|---|---|
//! | key exists on one machine at generation | never | never |
//! | key exists on one machine at signing | for one signature | never |
//! | `t-1` custodians learn the key | never — information-theoretically | never |
//! | a dishonest dealer is caught | yes, by every recipient, at deal time | yes |
//! | custodians talk to each other to sign | **no — one message each, one round** | usually several rounds |
//! | signature verifies under stock FIPS 204/205 | yes | scheme-dependent |
//!
//! The "non-interactive" line is the one worth noticing. The brief asked for
//! non-interactive threshold signing, and this is genuinely non-interactive
//! *for the custodians*: each sends one message and never speaks to another
//! custodian. Real threshold ML-DSA constructions are not — they need
//! preprocessing rounds because the rejection sampling in Fiat–Shamir-with-
//! aborts has to be agreed on. The interactivity did not vanish; it moved into
//! the combiner, which is exactly the trade this design makes.
//!
//! # The reconstruction is checked before it is used
//!
//! Interpolating from too few shares does not fail — it returns a different
//! secret. Every path here interpolates the blinding factor alongside the
//! secret and checks the pair against the vault's public commitment
//! ([`vss::check_opening`]) before a key is derived. A short quorum, a
//! corrupted share store, or a dealer whose shares were never on one polynomial
//! all stop there, rather than producing a signature under a key that owns
//! nothing.

use maya_crypto_pq::kem::ENCAPSULATION_KEY_LEN;
use zeroize::Zeroizing;

use crate::dkg::{CustodianShare, VaultId, VaultPolicy};
use crate::error::{CustodyError, Result};
use crate::hybrid::{ADDRESS_LEN, HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LEN, VaultKey};
use crate::seal::{self, CeremonyKey, SealedShare};
use crate::vss::{self, Commitments, ShareBody};

/// Domain string for a signing session's identifier.
const SESSION_DOMAIN: &[u8] = b"maya2c.custody-mpc.signing-session.v1";

/// Everything about a vault that is not secret.
///
/// Safe to publish, back up, and print on paper. Holding it lets anybody check
/// a quorum's work — the address is what a reconstruction is measured against —
/// and lets nobody sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultDescriptor {
    /// Which vault.
    pub id: VaultId,
    /// The `t`-of-`n` rule.
    pub policy: VaultPolicy,
    /// The summed commitments from the ceremony.
    pub commitments: Commitments,
    /// The encoded hybrid public key.
    pub public_key: [u8; HYBRID_PUBLIC_KEY_LEN],
    /// The address funds are held at.
    pub address: [u8; ADDRESS_LEN],
}

impl VaultDescriptor {
    /// Learns a vault's public key, which takes a quorum.
    ///
    /// # Why establishment needs a quorum at all
    ///
    /// The vault's public key is `ML-DSA-KeyGen(BLAKE3(…, secret))`. There is
    /// no way to compute it from the Pedersen commitments — that is the point
    /// of them being perfectly hiding — so the first quorum to convene is what
    /// learns the address. This runs the same reconstruction a signature does,
    /// and it is subject to the same caveat at the top of this module.
    ///
    /// Every later quorum can then *verify* rather than trust: it reconstructs
    /// and compares against this descriptor.
    ///
    /// # Errors
    ///
    /// As [`SigningSession`], plus [`CustodyError::Lattice`] if key generation
    /// fails.
    pub fn establish(shares: &[CustodianShare]) -> Result<Self> {
        let quorum = Quorum::gather(shares)?;
        let key = quorum.derive()?;
        Ok(Self {
            id: quorum.vault,
            policy: quorum.policy,
            commitments: quorum.commitments,
            public_key: key.public_key(),
            address: key.address(),
        })
    }
}

/// A checked set of contributions from distinct custodians of one vault.
struct Quorum {
    vault: VaultId,
    policy: VaultPolicy,
    commitments: Commitments,
    shares: Vec<ShareBody>,
}

impl Quorum {
    /// Checks a set of contributions and refuses anything that is not a quorum.
    fn gather(shares: &[CustodianShare]) -> Result<Self> {
        let first = shares.first().ok_or(CustodyError::ShortOfThreshold {
            received: 0,
            threshold: 0,
        })?;

        let mut seen: Vec<u8> = Vec::with_capacity(shares.len());
        for share in shares {
            if share.vault != first.vault {
                return Err(CustodyError::WrongVault);
            }
            if share.index == 0 {
                return Err(CustodyError::ReservedIndex);
            }
            if share.index > first.policy.custodians {
                return Err(CustodyError::UnknownCustodian {
                    index: share.index,
                    custodians: first.policy.custodians,
                });
            }
            if seen.contains(&share.index) {
                return Err(CustodyError::DuplicateContribution(share.index));
            }
            seen.push(share.index);
        }

        if shares.len() < usize::from(first.policy.threshold) {
            return Err(CustodyError::ShortOfThreshold {
                received: shares.len(),
                threshold: first.policy.threshold,
            });
        }

        Ok(Self {
            vault: first.vault,
            policy: first.policy,
            commitments: first.commitments.clone(),
            shares: shares.iter().map(|s| s.share.clone()).collect(),
        })
    }

    /// Reconstructs, checks against the commitments, and derives the key.
    ///
    /// The [`vss::check_opening`] call is the whole reason this is one function
    /// rather than two: there must be no way to obtain the reconstructed secret
    /// without having checked it.
    fn derive(&self) -> Result<VaultKey> {
        let (secret, blind) = vss::interpolate_opening(&self.shares)?;
        vss::check_opening(&secret, &blind, &self.commitments)?;

        let chain_key: Zeroizing<[u8; 32]> = crate::hybrid::chain_key_from_secret(&secret);
        VaultKey::from_chain_key(&chain_key)
    }
}

/// One signing ceremony: a message, and the custodians who turned up.
///
/// Contributions arrive one at a time and in any order, which is what lets a
/// caller run the collection against real custodians that may be slow, absent,
/// or answering a different vault. The session accumulates until [`Self::sign`]
/// is called; nothing is reconstructed before then.
#[derive(Debug)]
pub struct SigningSession {
    descriptor: VaultDescriptor,
    message: Vec<u8>,
    key: CeremonyKey,
    id: [u8; 32],
    contributors: Vec<u8>,
    shares: Vec<ShareBody>,
}

/// What a combiner asks custodians for.
///
/// Carries the combiner's ephemeral KEM key, so a response is sealed to *this
/// session* and to nobody else. A response recorded off the wire cannot be
/// replayed into a later session, because a later session has a different key
/// and a different [`SigningSession::id`].
#[derive(Clone)]
pub struct SigningRequest {
    /// Which vault is being asked to sign.
    pub vault: VaultId,
    /// The exact bytes to be signed.
    pub message: Vec<u8>,
    /// The combiner's ephemeral encapsulation key for this session.
    pub encapsulation_key: [u8; ENCAPSULATION_KEY_LEN],
}

impl core::fmt::Debug for SigningRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SigningRequest")
            .field("vault", &self.vault)
            .field("message", &format_args!("{} bytes", self.message.len()))
            .field("encapsulation_key", &"<1184 bytes>")
            .finish()
    }
}

impl SigningRequest {
    /// The session identifier this request fixes.
    ///
    /// Covers the vault, the combiner's key, and the message. Every sealed
    /// response is bound to it, so a custodian's contribution authorises
    /// exactly one message in exactly one session.
    ///
    /// That is a bound on *relays*, not on the combiner: once a quorum's shares
    /// are open in front of it, the combiner holds the vault secret and can
    /// sign whatever it likes. The module header does not hide this and neither
    /// does this sentence.
    #[must_use]
    pub fn id(&self) -> [u8; 32] {
        session_id(&self.vault, &self.encapsulation_key, &self.message)
    }
}

/// Seals one custodian's share as a response to `request`.
///
/// The custodian side of the network path. It checks the request names the
/// vault this share belongs to and nothing else — whether the message *should*
/// be signed is a policy question this crate has no opinion about, and an
/// approval workflow that pretended to live here would be a checkbox rather
/// than a control.
///
/// # Errors
///
/// - [`CustodyError::WrongVault`] if the request is for another vault.
/// - [`CustodyError::SealFailed`] if the combiner's key does not decode.
pub fn respond(request: &SigningRequest, held: &CustodianShare) -> Result<SealedShare> {
    if request.vault != held.vault {
        return Err(CustodyError::WrongVault);
    }
    seal::seal(
        &held.share,
        &request.encapsulation_key,
        &request.id(),
        held.index,
    )
}

fn session_id(
    vault: &VaultId,
    encapsulation_key: &[u8; ENCAPSULATION_KEY_LEN],
    message: &[u8],
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SESSION_DOMAIN);
    hasher.update(&vault.0);
    hasher.update(encapsulation_key);
    // Length-prefixed so two different messages cannot produce one id by
    // running into the field that follows. Nothing follows today; something
    // will.
    hasher.update(&(message.len() as u64).to_le_bytes());
    hasher.update(message);
    *hasher.finalize().as_bytes()
}

impl SigningSession {
    /// Opens a session to sign `message` under `descriptor`.
    #[must_use]
    pub fn open(descriptor: VaultDescriptor, message: Vec<u8>) -> Self {
        let capacity = usize::from(descriptor.policy.custodians);
        let key = CeremonyKey::generate();
        let id = session_id(&descriptor.id, &key.encapsulation_key(), &message);
        Self {
            descriptor,
            message,
            key,
            id,
            contributors: Vec::with_capacity(capacity),
            shares: Vec::with_capacity(capacity),
        }
    }

    /// This session's identifier.
    #[must_use]
    pub fn id(&self) -> [u8; 32] {
        self.id
    }

    /// The request to send to every custodian.
    #[must_use]
    pub fn request(&self) -> SigningRequest {
        SigningRequest {
            vault: self.descriptor.id,
            message: self.message.clone(),
            encapsulation_key: self.key.encapsulation_key(),
        }
    }

    /// Opens a sealed response and folds it in.
    ///
    /// # Errors
    ///
    /// [`CustodyError::SealFailed`] if the response was not sealed to this
    /// session — a response to a different session, or a different vault, or a
    /// different message, fails here rather than quietly joining the quorum.
    /// Plus everything [`Self::contribute`] can return.
    pub fn accept_sealed(&mut self, sealed: &SealedShare) -> Result<()> {
        let share = seal::open(sealed, &self.key, &self.id)?;
        if share.index != sealed.dealer {
            return Err(CustodyError::SealFailed(sealed.dealer));
        }
        self.add(share)
    }

    /// Adds one custodian's contribution.
    ///
    /// Contributions past the threshold are accepted rather than refused: a
    /// session that rejected the fourth of five responders would turn a healthy
    /// quorum into an error, and the extra share costs one field multiply.
    ///
    /// # Errors
    ///
    /// - [`CustodyError::WrongVault`] for a share of a different vault.
    /// - [`CustodyError::ReservedIndex`] or [`CustodyError::UnknownCustodian`]
    ///   for an index outside the roster.
    /// - [`CustodyError::DuplicateContribution`] if this custodian already
    ///   contributed. A "quorum" that is one custodian three times is not a
    ///   quorum, and it is also a division by zero in the interpolation.
    pub fn contribute(&mut self, contribution: &CustodianShare) -> Result<()> {
        if contribution.vault != self.descriptor.id {
            return Err(CustodyError::WrongVault);
        }
        if contribution.index == 0 {
            return Err(CustodyError::ReservedIndex);
        }
        if contribution.index > self.descriptor.policy.custodians {
            return Err(CustodyError::UnknownCustodian {
                index: contribution.index,
                custodians: self.descriptor.policy.custodians,
            });
        }
        self.add(contribution.share.clone())
    }

    /// The one path by which a share joins a quorum.
    fn add(&mut self, share: ShareBody) -> Result<()> {
        if share.index == 0 {
            return Err(CustodyError::ReservedIndex);
        }
        if share.index > self.descriptor.policy.custodians {
            return Err(CustodyError::UnknownCustodian {
                index: share.index,
                custodians: self.descriptor.policy.custodians,
            });
        }
        if self.contributors.contains(&share.index) {
            return Err(CustodyError::DuplicateContribution(share.index));
        }

        self.contributors.push(share.index);
        self.shares.push(share);
        Ok(())
    }

    /// The custodians who have contributed so far, in arrival order.
    #[must_use]
    pub fn contributors(&self) -> &[u8] {
        &self.contributors
    }

    /// Whether enough custodians have contributed to sign.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.contributors.len() >= usize::from(self.descriptor.policy.threshold)
    }

    /// Reconstructs, checks, signs, and drops the key.
    ///
    /// # Errors
    ///
    /// - [`CustodyError::ShortOfThreshold`] if too few custodians answered —
    ///   the crash-fault case, reported with both numbers so an operator knows
    ///   how many more to wake.
    /// - [`CustodyError::WrongSeed`] if the reconstruction does not open the
    ///   vault's commitment, or derives an address that is not the vault's.
    /// - [`CustodyError::Lattice`] if ML-DSA signing fails.
    pub fn sign(&self) -> Result<[u8; HYBRID_SIGNATURE_LEN]> {
        if !self.is_ready() {
            return Err(CustodyError::ShortOfThreshold {
                received: self.contributors.len(),
                threshold: self.descriptor.policy.threshold,
            });
        }

        let (secret, blind) = vss::interpolate_opening(&self.shares)?;
        vss::check_opening(&secret, &blind, &self.descriptor.commitments)?;

        let chain_key = crate::hybrid::chain_key_from_secret(&secret);
        let key = VaultKey::from_chain_key(&chain_key)?;

        // Belt and braces over `check_opening`, and cheap. The commitment check
        // proves the quorum reconstructed *the* vault secret; this proves the
        // descriptor being signed under is the one that secret belongs to. They
        // fail together in every case anyone has thought of, and the one nobody
        // has thought of is why both are here.
        if key.address() != self.descriptor.address {
            return Err(CustodyError::WrongSeed);
        }

        key.sign(&self.message)
    }
}
