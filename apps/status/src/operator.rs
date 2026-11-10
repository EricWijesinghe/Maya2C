//! The operator's ML-DSA-65 key: the only thing that can publish an incident.
//!
//! The secret is a 32-byte seed, the same derivation `apps/chat` uses, kept
//! as a hex file outside the repository. The expanded key zeroizes on drop.

use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use zeroize::Zeroizing;

/// Length of the seed file's secret, in bytes.
pub const SEED_LEN: usize = 32;

/// An operator's signing key and its public half.
pub struct Operator {
    key: <MlDsa65 as SignatureSuite>::SigningKey,
    public: Vec<u8>,
}

impl Operator {
    /// The operator a seed names.
    #[must_use]
    pub fn from_seed(seed: [u8; SEED_LEN]) -> Self {
        let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(seed));
        let public = MlDsa65::public_key(&key);
        Self { key, public }
    }

    /// A fresh seed from the operating system, and the operator it names.
    ///
    /// # Errors
    ///
    /// The operating system refused to supply entropy.
    pub fn generate() -> Result<(Zeroizing<[u8; SEED_LEN]>, Self), String> {
        let mut seed = Zeroizing::new([0u8; SEED_LEN]);
        getrandom::fill(seed.as_mut()).map_err(|e| format!("entropy: {e}"))?;
        let op = Self::from_seed(*seed);
        Ok((seed, op))
    }

    /// The public key, the bytes an operator list names.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// Signs `message` with the operator's key.
    ///
    /// # Errors
    ///
    /// The signing backend refused the message.
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
        MlDsa65::sign(&self.key, message).map_err(|e| format!("sign: {e}"))
    }
}

/// Whether `signature` is `public`'s signature over `message`.
#[must_use]
pub fn verify(public: &[u8], message: &[u8], signature: &[u8]) -> bool {
    MlDsa65::verify(public, message, signature).is_ok()
}

/// The seed in a key file: one line of hex, 32 bytes.
///
/// # Errors
///
/// The text is not 64 hex digits.
pub fn parse_seed(text: &str) -> Result<Zeroizing<[u8; SEED_LEN]>, String> {
    let bytes = hex::decode(text.trim()).map_err(|_| "key file is not hex".to_owned())?;
    let seed: [u8; SEED_LEN] = bytes
        .try_into()
        .map_err(|_| format!("key file must hold {SEED_LEN} bytes"))?;
    Ok(Zeroizing::new(seed))
}

/// The operator list: one hex public key per line; `#` starts a comment.
///
/// # Errors
///
/// A line that is not hex.
pub fn parse_operators(text: &str) -> Result<Vec<Vec<u8>>, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| hex::decode(l).map_err(|_| format!("operator key is not hex: {l:.16}...")))
        .collect()
}

/// The operator list at `path`. A missing file is an empty list, so nothing
/// verifies as published until an operator adds a key.
///
/// # Errors
///
/// The file exists but cannot be read or parsed.
pub fn read_operators(path: &std::path::Path) -> Result<Vec<Vec<u8>>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_operators(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_signature_verifies_under_its_own_key_only() {
        let a = Operator::from_seed([1; SEED_LEN]);
        let b = Operator::from_seed([2; SEED_LEN]);
        let sig = a.sign(b"msg").unwrap();
        assert!(verify(a.public_key(), b"msg", &sig));
        assert!(!verify(a.public_key(), b"other", &sig));
        assert!(!verify(b.public_key(), b"msg", &sig));
    }

    #[test]
    fn seed_files_must_hold_exactly_32_bytes_of_hex() {
        assert!(parse_seed(&hex::encode([7u8; SEED_LEN])).is_ok());
        assert!(parse_seed("abcd").is_err());
        assert!(parse_seed("zz").is_err());
    }

    #[test]
    fn operator_lists_skip_comments_and_blank_lines() {
        let keys = parse_operators("# ops\n\n0a0b\n  0c0d  \n").unwrap();
        assert_eq!(keys, vec![vec![0x0a, 0x0b], vec![0x0c, 0x0d]]);
        assert!(parse_operators("nothex").is_err());
    }
}
