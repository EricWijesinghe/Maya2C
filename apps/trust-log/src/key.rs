//! The foundation's signing key: an ML-DSA-87 key expanded from a 32-byte
//! seed. The seed is held in `Zeroizing` buffers from file to key, the key is
//! `ZeroizeOnDrop` (the `SignatureSuite` bound), and neither is ever printed.

use std::path::Path;

use maya_crypto_pq::suite::{self, MasterSeed, MlDsa87, SignatureSuite, SuiteId};
use zeroize::Zeroizing;

use crate::{Error, Result};

/// Seed length in bytes.
pub const SEED_LEN: usize = 32;

/// A signing key and its public half.
pub struct SignerKey {
    key: <MlDsa87 as SignatureSuite>::SigningKey,
    public: Vec<u8>,
}

impl core::fmt::Debug for SignerKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SignerKey(<redacted>)")
    }
}

impl SignerKey {
    /// The key a seed expands to.
    #[must_use]
    pub fn from_seed(seed: &Zeroizing<[u8; SEED_LEN]>) -> Self {
        let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes(**seed));
        let public = MlDsa87::public_key(&key);
        Self { key, public }
    }

    /// A fresh key, and the seed to store it under.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the operating system has no entropy to give.
    pub fn generate() -> Result<(Self, Zeroizing<[u8; SEED_LEN]>)> {
        let mut seed = Zeroizing::new([0u8; SEED_LEN]);
        getrandom::fill(seed.as_mut()).map_err(|e| Error::Io(format!("entropy: {e}")))?;
        Ok((Self::from_seed(&seed), seed))
    }

    /// Reads a hex seed file.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if unreadable, [`Error::Malformed`] if not 32 bytes of
    /// hex. Neither message quotes the file's contents.
    pub fn load(path: &Path) -> Result<Self> {
        let text = Zeroizing::new(
            std::fs::read_to_string(path)
                .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?,
        );
        let bytes = Zeroizing::new(
            hex::decode(text.trim())
                .map_err(|_| Error::Malformed("seed file is not hex".into()))?,
        );
        let mut seed = Zeroizing::new([0u8; SEED_LEN]);
        if bytes.len() != SEED_LEN {
            return Err(Error::Malformed(format!(
                "seed file holds {} bytes, want {SEED_LEN}",
                bytes.len()
            )));
        }
        seed.copy_from_slice(&bytes);
        Ok(Self::from_seed(&seed))
    }

    /// The encoded public key.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// The chain account this key controls as a suite-tagged ML-DSA-87 key:
    /// the sender an anchor transaction signed by it carries.
    #[must_use]
    pub fn address(&self) -> [u8; 32] {
        address_of(&self.public)
    }

    /// Signs `message`.
    ///
    /// # Errors
    ///
    /// [`Error::Signature`] if the backend refuses.
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        MlDsa87::sign(&self.key, message).map_err(|e| Error::Signature(e.to_string()))
    }
}

/// The account address of an ML-DSA-87 public key.
#[must_use]
pub fn address_of(public_key: &[u8]) -> [u8; 32] {
    suite::suite_address(SuiteId::MlDsa87, public_key)
}

/// Writes `seed` as hex, refusing to overwrite an existing file.
///
/// # Errors
///
/// [`Error::Io`] if the file exists or cannot be written.
pub fn write_seed(path: &Path, seed: &Zeroizing<[u8; SEED_LEN]>) -> Result<()> {
    use std::io::Write;
    let text = Zeroizing::new(hex::encode(seed.as_ref()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    file.write_all(text.as_bytes())
        .map_err(|e| Error::Io(format!("{}: {e}", path.display())))
}
