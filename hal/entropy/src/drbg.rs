//! `HMAC_DRBG` with HMAC-SHA-256, NIST SP 800-90A Rev. 1 §10.1.2.
//!
//! Security strength 256 bits. Validated against the NIST CAVP
//! `HMAC_DRBG.rsp` vectors (`tests/drbg_cavp_tests.rs`), both the no-reseed
//! and the prediction-resistance-false files.
//!
//! The state (`K`, `V`) is wiped on drop and on [`HmacDrbg::zeroize`], which is
//! what the tamper line calls.

use hmac::{Hmac, Mac as _};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

type HmacSha256 = Hmac<Sha256>;

const OUT_LEN: usize = 32;

/// Minimum entropy input for 256-bit security strength, SP 800-90A §10.1.
pub const MIN_ENTROPY_LEN: usize = 32;

/// Largest single request, 2¹⁹ bits (Table 2).
pub const MAX_REQUEST_BYTES: usize = 1 << 16;

/// Generate calls allowed between reseeds. The standard permits 2⁴⁸; the
/// pool reseeds far more often than this, from fresh source output.
pub const RESEED_INTERVAL: u64 = 1 << 48;

/// DRBG errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DrbgError {
    /// Fewer than 32 bytes of entropy input.
    #[error("entropy input shorter than {MIN_ENTROPY_LEN} bytes")]
    InsufficientEntropy,
    /// More than 2¹⁹ bits requested in one call.
    #[error("request longer than {MAX_REQUEST_BYTES} bytes")]
    RequestTooLarge,
    /// The reseed counter has run out.
    #[error("reseed required")]
    ReseedRequired,
}

/// The DRBG working state.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct HmacDrbg {
    k: [u8; OUT_LEN],
    v: [u8; OUT_LEN],
    reseed_counter: u64,
}

impl core::fmt::Debug for HmacDrbg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "HmacDrbg {{ reseed_counter: {}, state: <redacted> }}",
            self.reseed_counter
        )
    }
}

fn hmac(key: &[u8; OUT_LEN], parts: &[&[u8]]) -> [u8; OUT_LEN] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

impl HmacDrbg {
    /// `HMAC_DRBG_Instantiate`: `entropy ‖ nonce ‖ personalization` seeds it.
    ///
    /// # Errors
    ///
    /// [`DrbgError::InsufficientEntropy`] below 32 bytes of entropy.
    pub fn instantiate(
        entropy: &[u8],
        nonce: &[u8],
        personalization: &[u8],
    ) -> Result<Self, DrbgError> {
        if entropy.len() < MIN_ENTROPY_LEN {
            return Err(DrbgError::InsufficientEntropy);
        }
        let mut drbg = Self {
            k: [0x00; OUT_LEN],
            v: [0x01; OUT_LEN],
            reseed_counter: 1,
        };
        drbg.update(&[entropy, nonce, personalization]);
        Ok(drbg)
    }

    /// `HMAC_DRBG_Reseed`.
    ///
    /// # Errors
    ///
    /// [`DrbgError::InsufficientEntropy`] below 32 bytes of entropy.
    pub fn reseed(&mut self, entropy: &[u8], additional: &[u8]) -> Result<(), DrbgError> {
        if entropy.len() < MIN_ENTROPY_LEN {
            return Err(DrbgError::InsufficientEntropy);
        }
        self.update(&[entropy, additional]);
        self.reseed_counter = 1;
        Ok(())
    }

    /// `HMAC_DRBG_Generate`: fills `out`.
    ///
    /// # Errors
    ///
    /// A request over 2¹⁹ bits, or an exhausted reseed counter.
    pub fn generate(&mut self, out: &mut [u8], additional: &[u8]) -> Result<(), DrbgError> {
        if out.len() > MAX_REQUEST_BYTES {
            return Err(DrbgError::RequestTooLarge);
        }
        if self.reseed_counter > RESEED_INTERVAL {
            return Err(DrbgError::ReseedRequired);
        }
        if !additional.is_empty() {
            self.update(&[additional]);
        }
        for chunk in out.chunks_mut(OUT_LEN) {
            self.v = hmac(&self.k, &[&self.v]);
            chunk.copy_from_slice(&self.v[..chunk.len()]);
        }
        self.update(&[additional]);
        self.reseed_counter += 1;
        Ok(())
    }

    /// Calls since the last (re)seed.
    #[must_use]
    pub fn reseed_counter(&self) -> u64 {
        self.reseed_counter
    }

    /// `HMAC_DRBG_Update` over the concatenation of `provided`.
    fn update(&mut self, provided: &[&[u8]]) {
        let empty = provided.iter().all(|p| p.is_empty());
        for round in [0x00u8, 0x01] {
            if round == 0x01 && empty {
                return;
            }
            let mut parts: Vec<&[u8]> = vec![&self.v, core::slice::from_ref(&round)];
            parts.extend_from_slice(provided);
            self.k = hmac(&self.k, &parts);
            self.v = hmac(&self.k, &[&self.v]);
        }
    }
}
