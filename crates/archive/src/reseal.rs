//! Re-sealing archive roots when a seal algorithm ages (`docs/resealing.md`,
//! Procedure A; Master Prompt 28 §3).
//!
//! **Never replace evidence, add to it.** A re-seal signs, under a successor
//! suite, the root *and the complete previous evidence* — the original
//! [`Seal`] or an earlier re-seal. Layers nest, so an archive sealed in 2026
//! under SLH-DSA-SHAKE-256f, re-sealed under a SHA-2 suite when SHAKE is
//! doubted, and again under a lattice suite later, carries all three, and a
//! verifier checks whichever layers it still trusts.
//!
//! The successor suite is any registry suite (`maya_crypto_pq::suite`),
//! recorded in the re-seal so a verifier needs no out-of-band hint. Which one
//! to use when is an ADR at the time of the event; a successor chain's key is
//! published out of band, never certified by the doubted chain.

use std::collections::BTreeMap;

use maya_crypto_pq::agility::{SuitePolicy, SuiteStatus};
use maya_crypto_pq::suite::{self, MasterSeed, SignatureSuite, SuiteId};

use crate::error::{ArchiveError, Result};
use crate::seal::{KeyChain, Seal};

const RESEAL_DOMAIN: &[u8] = b"maya2c archive reseal v1";
const TAG_SEAL: u8 = 0;
const TAG_RESEAL: u8 = 1;
/// Deepest nesting a decoder accepts: one layer per algorithm generation.
pub const MAX_LAYERS: usize = 16;

/// The evidence attached to an archive root: an original seal, or a re-seal
/// wrapping earlier evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// The original forward-secure SLH-DSA seal.
    Seal(Seal),
    /// A successor suite's signature over the root and the inner evidence.
    Reseal(Box<Reseal>),
}

/// One re-seal layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reseal {
    /// The successor suite that signed.
    pub suite: SuiteId,
    /// The archive root (must equal the inner evidence's root).
    pub root: Vec<u8>,
    /// What this layer wraps.
    pub inner: Evidence,
    /// The signature over domain, suite, root and the encoded inner evidence.
    pub signature: Vec<u8>,
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(bytes);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(ArchiveError::Seal("truncated evidence".into()));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| ArchiveError::Seal("u64".into()))?,
        ))
    }
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| ArchiveError::Seal("length".into()))?,
        );
        Ok(self
            .take(usize::try_from(n).unwrap_or(usize::MAX))?
            .to_vec())
    }
}

impl Evidence {
    /// The root this evidence covers.
    #[must_use]
    pub fn root(&self) -> &[u8] {
        match self {
            Self::Seal(s) => &s.root,
            Self::Reseal(r) => &r.root,
        }
    }

    /// Re-seal layers above the original seal.
    #[must_use]
    pub fn layers(&self) -> usize {
        match self {
            Self::Seal(_) => 0,
            Self::Reseal(r) => 1 + r.inner.layers(),
        }
    }

    /// The canonical encoding a re-seal signs over.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Seal(s) => {
                out.push(TAG_SEAL);
                out.extend_from_slice(&s.epoch.to_le_bytes());
                put(&mut out, &s.root);
                put(&mut out, &s.signature);
            }
            Self::Reseal(r) => {
                out.push(TAG_RESEAL);
                out.push(r.suite.to_byte());
                put(&mut out, &r.root);
                put(&mut out, &r.inner.encode());
                put(&mut out, &r.signature);
            }
        }
        out
    }

    /// Decodes [`Evidence::encode`]'s output.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Seal`] for malformed bytes, an unknown suite, or nesting
    /// deeper than [`MAX_LAYERS`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader(bytes);
        let evidence = Self::read(&mut r, 0)?;
        if !r.0.is_empty() {
            return Err(ArchiveError::Seal("trailing bytes after evidence".into()));
        }
        Ok(evidence)
    }

    fn read(r: &mut Reader<'_>, depth: usize) -> Result<Self> {
        if depth > MAX_LAYERS {
            return Err(ArchiveError::Seal("evidence nested too deeply".into()));
        }
        match r.byte()? {
            TAG_SEAL => Ok(Self::Seal(Seal {
                epoch: r.u64()?,
                root: r.bytes()?,
                signature: r.bytes()?,
            })),
            TAG_RESEAL => {
                let suite = SuiteId::try_from(r.byte()?)
                    .map_err(|e| ArchiveError::Seal(format!("{e:?}")))?;
                let root = r.bytes()?;
                let inner_bytes = r.bytes()?;
                let mut inner_reader = Reader(&inner_bytes);
                let inner = Self::read(&mut inner_reader, depth + 1)?;
                // The signature covers the inner evidence re-encoded, so bytes
                // after it would ride along unsigned: one encoding per evidence.
                if !inner_reader.0.is_empty() {
                    return Err(ArchiveError::Seal("trailing bytes inside a layer".into()));
                }
                Ok(Self::Reseal(Box::new(Reseal {
                    suite,
                    root,
                    inner,
                    signature: r.bytes()?,
                })))
            }
            t => Err(ArchiveError::Seal(format!("evidence tag {t}"))),
        }
    }
}

fn reseal_message(suite: SuiteId, root: &[u8], inner: &Evidence) -> Vec<u8> {
    let mut m = RESEAL_DOMAIN.to_vec();
    m.push(suite.to_byte());
    put(&mut m, root);
    put(&mut m, &inner.encode());
    m
}

/// A successor chain's signer, its key published out of band.
pub struct Resealer<S: SignatureSuite> {
    key: S::SigningKey,
    public_key: Vec<u8>,
}

impl<S: SignatureSuite> Resealer<S> {
    /// A resealer from a seed held like any sealer's.
    #[must_use]
    pub fn new(seed: &MasterSeed) -> Self {
        let key = S::signing_key_from_seed(seed);
        let public_key = S::public_key(&key);
        Self { key, public_key }
    }

    /// The key to publish.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// Wraps `evidence` in a new layer. Nothing is replaced.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Seal`] if the evidence already has [`MAX_LAYERS`]
    /// layers (one more could be written but never decoded), or signing fails.
    pub fn reseal(&self, evidence: Evidence) -> Result<Evidence> {
        if evidence.layers() >= MAX_LAYERS {
            return Err(ArchiveError::Seal(format!(
                "evidence already has {MAX_LAYERS} layers"
            )));
        }
        let root = evidence.root().to_vec();
        let signature = S::sign(&self.key, &reseal_message(S::ID, &root, &evidence))
            .map_err(|e| ArchiveError::Seal(format!("reseal: {e:?}")))?;
        Ok(Evidence::Reseal(Box::new(Reseal {
            suite: S::ID,
            root,
            inner: evidence,
            signature,
        })))
    }
}

/// What a verifier trusts: the original seal chain, and one published key per
/// successor suite.
#[derive(Clone, Debug, Default)]
pub struct Trust {
    /// The original SLH-DSA seal chain, if it is still trusted.
    pub seals: Option<KeyChain>,
    /// Successor keys by suite.
    pub resealers: BTreeMap<SuiteId, Vec<u8>>,
}

/// Checks every layer of `evidence` over `root`, outermost first, and returns
/// the suites whose layers verified. A layer under a suite the verifier does
/// not trust is skipped, not failed: that is the point of adding evidence
/// rather than replacing it. A layer that is present, trusted and wrong, or a
/// root that changes between layers, fails the whole check.
///
/// # Errors
///
/// [`ArchiveError::Seal`] for a bad signature, a root mismatch, or evidence
/// in which no trusted layer verified.
pub fn verify(evidence: &Evidence, root: &[u8], trust: &Trust) -> Result<Vec<SuiteId>> {
    let mut verified = Vec::new();
    let mut layer = evidence;
    loop {
        if layer.root() != root {
            return Err(ArchiveError::Seal("a layer covers a different root".into()));
        }
        match layer {
            Evidence::Reseal(r) => {
                if let Some(key) = trust.resealers.get(&r.suite) {
                    suite::verify(
                        r.suite,
                        key,
                        &reseal_message(r.suite, &r.root, &r.inner),
                        &r.signature,
                    )
                    .map_err(|e| {
                        ArchiveError::Seal(format!("reseal under {:?}: {e:?}", r.suite))
                    })?;
                    verified.push(r.suite);
                }
                layer = &r.inner;
            }
            Evidence::Seal(s) => {
                if let Some(chain) = &trust.seals {
                    chain.verify(s)?;
                    verified.push(crate::seal::SEAL_SUITE);
                }
                break;
            }
        }
    }
    if verified.is_empty() {
        return Err(ArchiveError::Seal("no layer is under a trusted key".into()));
    }
    Ok(verified)
}

/// Whether an archive's evidence needs a new layer, under the suite policy
/// governance maintains (`maya_crypto_pq::agility`).
///
/// Only the outermost layer matters: the inner ones are history, and the
/// point of a new layer is that nobody has to trust them any more. The rule
/// is deterministic in `(evidence, policy, height)`, so every custodian
/// running it against the same chain height reaches the same answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResealDue {
    /// The outermost suite is active.
    NotDue,
    /// Deprecated: re-seal before `sunset_height`.
    Due {
        /// The height after which the outer layer no longer counts as live.
        sunset_height: u64,
    },
    /// Sunset or never permitted: the archive is resting on a suite the
    /// network no longer signs with. Re-seal now.
    Overdue,
}

/// The suite that made the outermost layer.
#[must_use]
pub fn outer_suite(evidence: &Evidence) -> SuiteId {
    match evidence {
        Evidence::Seal(_) => crate::seal::SEAL_SUITE,
        Evidence::Reseal(r) => r.suite,
    }
}

/// When `evidence` must next be re-sealed.
#[must_use]
pub fn schedule(evidence: &Evidence, policy: &SuitePolicy, height: u64) -> ResealDue {
    match policy.status(outer_suite(evidence), height) {
        SuiteStatus::Active => ResealDue::NotDue,
        SuiteStatus::Deprecated { sunset_height } => ResealDue::Due { sunset_height },
        SuiteStatus::Sunset | SuiteStatus::Forbidden => ResealDue::Overdue,
    }
}

/// Re-seals only when [`schedule`] says so, and only under a successor the
/// policy still treats as active — a re-seal under a doubted suite adds
/// nothing. Returns the evidence unchanged when nothing is due.
///
/// # Errors
///
/// [`ArchiveError::Seal`] when the successor suite is not active at `height`,
/// or signing fails.
pub fn reseal_if_due<S: SignatureSuite>(
    resealer: &Resealer<S>,
    evidence: Evidence,
    policy: &SuitePolicy,
    height: u64,
) -> Result<Evidence> {
    if schedule(&evidence, policy, height) == ResealDue::NotDue {
        return Ok(evidence);
    }
    if policy.status(S::ID, height) != SuiteStatus::Active {
        return Err(ArchiveError::Seal(format!(
            "successor {:?} is not active at height {height}",
            S::ID
        )));
    }
    resealer.reseal(evidence)
}
