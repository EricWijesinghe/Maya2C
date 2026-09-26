//! Forward-secure SLH-DSA seals over archive roots.
//!
//! # What a seal is for
//!
//! An archive's root CID already proves its bytes are the bytes it names. A
//! seal adds *who* produced the batch, and *when* in the sealer's key
//! history: an SLH-DSA-SHAKE-256f signature by the key of one **epoch** over
//! that epoch and the root. SLH-DSA because an archive is read for decades,
//! and a hash-based signature rests on nothing but its hash function (FIPS
//! 205, NIST category 5).
//!
//! # Forward security
//!
//! The sealer keeps one 32-byte chain seed. Epoch `e + 1`'s seed is
//! `SHAKE256(EVOLVE ‖ seed_e)`, and [`EpochSigner::evolve`] consumes the old
//! signer, so its seed and key are zeroized on drop. An attacker who takes
//! the machine at epoch `e` can seal as `e` and later, which a revocation can
//! then cut off. They cannot seal as any earlier epoch: that needs a SHAKE256
//! preimage. Each epoch's signing key comes from its seed under a separate
//! domain, so no key material reveals the next seed.
//!
//! # How a verifier trusts epoch `e`'s key
//!
//! Before its key is erased, each epoch signs a [`Transition`] certifying the
//! next epoch's public key. [`KeyChain`] starts from the genesis public key
//! alone and admits a transition only if the current tip signed it, so a
//! verifier holding one key follows the whole history, with no horizon fixed
//! in advance. When the algorithm itself ages, `docs/resealing.md` says what
//! happens.

use maya_crypto_pq::suite::{MasterSeed, SignatureSuite, SlhDsaShake256f, SuiteId};
use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};
use zeroize::Zeroizing;

use crate::error::{ArchiveError, Result};

/// The suite every seal and transition is signed under.
pub const SEAL_SUITE: SuiteId = SuiteId::SlhDsaShake256f;

const EVOLVE_DOMAIN: &[u8] = b"maya2c archive seal: evolve chain seed v1";
const KEY_DOMAIN: &[u8] = b"maya2c archive seal: epoch signing key v1";
const SEAL_DOMAIN: &[u8] = b"maya2c archive seal: seal v1";
const TRANSITION_DOMAIN: &[u8] = b"maya2c archive seal: transition v1";

type SigningKey = <SlhDsaShake256f as SignatureSuite>::SigningKey;

/// SHAKE256 over `parts`, 32 bytes, straight into a zeroizing buffer.
fn shake(parts: &[&[u8]]) -> Zeroizing<[u8; 32]> {
    let mut xof = Shake256::default();
    for part in parts {
        xof.update(part);
    }
    let mut out = Zeroizing::new([0u8; 32]);
    xof.finalize_xof().read(out.as_mut_slice());
    out
}

/// The sealer for one epoch. Neither `Clone` nor `Debug`: one copy, erased
/// by [`EpochSigner::evolve`].
pub struct EpochSigner {
    epoch: u64,
    seed: Zeroizing<[u8; 32]>,
    key: SigningKey,
    public_key: Vec<u8>,
}

impl EpochSigner {
    /// Epoch 0, from a fresh master seed.
    #[must_use]
    pub fn genesis(seed: &MasterSeed) -> Self {
        Self::at(0, Zeroizing::new(*seed.expose()))
    }

    fn at(epoch: u64, seed: Zeroizing<[u8; 32]>) -> Self {
        let key_seed = shake(&[KEY_DOMAIN, &epoch.to_le_bytes(), seed.as_slice()]);
        let key = SlhDsaShake256f::signing_key_from_seed(&MasterSeed::from_bytes(*key_seed));
        let public_key = SlhDsaShake256f::public_key(&key);
        Self {
            epoch,
            seed,
            key,
            public_key,
        }
    }

    /// The epoch this signer seals as.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// This epoch's public key.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// Seals `root` (an archive's root CID bytes) as this epoch.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Seal`] if the signer refuses.
    pub fn seal(&self, root: &[u8]) -> Result<Seal> {
        let signature = SlhDsaShake256f::sign(&self.key, &seal_message(self.epoch, root))
            .map_err(|e| ArchiveError::Seal(e.to_string()))?;
        Ok(Seal {
            epoch: self.epoch,
            root: root.to_vec(),
            signature,
        })
    }

    /// Moves to the next epoch: certifies its key, then drops this one.
    ///
    /// Consuming `self` is the forward-security guarantee. Once this returns,
    /// no value in the process holds this epoch's seed or key.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Seal`] if the epoch counter is exhausted or signing
    /// fails; `self` is dropped either way.
    pub fn evolve(self) -> Result<(Self, Transition)> {
        let epoch = self
            .epoch
            .checked_add(1)
            .ok_or_else(|| ArchiveError::Seal("epoch counter exhausted".into()))?;
        let next = Self::at(epoch, shake(&[EVOLVE_DOMAIN, self.seed.as_slice()]));
        let signature =
            SlhDsaShake256f::sign(&self.key, &transition_message(epoch, &next.public_key))
                .map_err(|e| ArchiveError::Seal(e.to_string()))?;
        let transition = Transition {
            epoch,
            public_key: next.public_key.clone(),
            signature,
        };
        Ok((next, transition))
    }
}

/// An epoch's seal over an archive root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seal {
    /// The sealing epoch.
    pub epoch: u64,
    /// The root it covers.
    pub root: Vec<u8>,
    /// SLH-DSA-SHAKE-256f over the domain, epoch and root.
    pub signature: Vec<u8>,
}

/// Epoch `epoch - 1` certifying epoch `epoch`'s public key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    /// The epoch being certified.
    pub epoch: u64,
    /// Its public key.
    pub public_key: Vec<u8>,
    /// The previous epoch's signature over both.
    pub signature: Vec<u8>,
}

fn seal_message(epoch: u64, root: &[u8]) -> Vec<u8> {
    [SEAL_DOMAIN, &epoch.to_le_bytes(), root].concat()
}

fn transition_message(epoch: u64, public_key: &[u8]) -> Vec<u8> {
    [TRANSITION_DOMAIN, &epoch.to_le_bytes(), public_key].concat()
}

/// A verifier's view: the genesis key and every certified transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyChain {
    keys: Vec<Vec<u8>>,
}

impl KeyChain {
    /// A chain holding only the key trusted out of band.
    #[must_use]
    pub fn new(genesis_public_key: Vec<u8>) -> Self {
        Self {
            keys: vec![genesis_public_key],
        }
    }

    /// The epoch of the newest certified key.
    #[must_use]
    pub fn tip(&self) -> u64 {
        u64::try_from(self.keys.len() - 1).unwrap_or(u64::MAX)
    }

    /// Admits `transition` if the tip signed it and it names the next epoch.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Seal`] for a skipped or repeated epoch, or a signature
    /// the tip's key did not make.
    pub fn admit(&mut self, transition: Transition) -> Result<()> {
        if Some(transition.epoch) != self.tip().checked_add(1) {
            return Err(ArchiveError::Seal(format!(
                "transition to epoch {} after tip {}",
                transition.epoch,
                self.tip()
            )));
        }
        let tip_key = self
            .keys
            .last()
            .ok_or_else(|| ArchiveError::Seal("empty chain".into()))?;
        SlhDsaShake256f::verify(
            tip_key,
            &transition_message(transition.epoch, &transition.public_key),
            &transition.signature,
        )
        .map_err(|e| ArchiveError::Seal(format!("transition {}: {e}", transition.epoch)))?;
        self.keys.push(transition.public_key);
        Ok(())
    }

    /// Epoch `epoch`'s certified key.
    #[must_use]
    pub fn key(&self, epoch: u64) -> Option<&[u8]> {
        self.keys
            .get(usize::try_from(epoch).ok()?)
            .map(Vec::as_slice)
    }

    /// Checks `seal` against its epoch's certified key.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Seal`] for an uncertified epoch or a bad signature.
    pub fn verify(&self, seal: &Seal) -> Result<()> {
        let key = self
            .key(seal.epoch)
            .ok_or_else(|| ArchiveError::Seal(format!("epoch {} is not certified", seal.epoch)))?;
        SlhDsaShake256f::verify(key, &seal_message(seal.epoch, &seal.root), &seal.signature)
            .map_err(|e| ArchiveError::Seal(format!("seal at epoch {}: {e}", seal.epoch)))
    }
}
