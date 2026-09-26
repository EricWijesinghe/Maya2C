//! Compact radio frames (Master Prompt 7 §4).
//!
//! A post-quantum signature is 3,309 bytes (ML-DSA-65) to 4,627 bytes
//! (ML-DSA-87). A `LoRa` frame is 222 bytes at most and the brief's compact
//! frame is under 256. **A PQ signature cannot fit in a radio frame**, and
//! no encoding changes that. So a radio frame carries the transaction's hash
//! and the gateway fetches the signature on demand over whatever wider link
//! it has, or aggregates at the gateway. This module is that rule as code.

/// Largest compact frame.
pub const MAX_COMPACT_FRAME: usize = 255;
/// ML-DSA-65 signature size (FIPS 204 final).
pub const ML_DSA_65_SIG: usize = 3_309;
/// ML-DSA-87 signature size (FIPS 204 final).
pub const ML_DSA_87_SIG: usize = 4_627;

/// What goes over the air.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RadioFrame {
    /// A reference to a transaction the gateway must fetch in full.
    TxRef {
        /// Transaction hash.
        hash: [u8; 32],
        /// Sender's account id (key hash), so the gateway can pre-route.
        sender: [u8; 32],
    },
    /// Payload small enough to send whole (no signature inside).
    Small(Vec<u8>),
}

impl RadioFrame {
    /// Encoded length.
    pub fn len(&self) -> usize {
        match self {
            Self::TxRef { .. } => 1 + 32 + 32,
            Self::Small(b) => 1 + b.len(),
        }
    }

    /// Whether it is empty (never, for a valid frame).
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// Frames a signed transaction for radio: whole if it fits, otherwise a
/// reference. A signed PQ transaction never fits.
pub fn frame_signed_tx(tx_bytes: &[u8], hash: [u8; 32], sender: [u8; 32]) -> RadioFrame {
    if tx_bytes.len() < MAX_COMPACT_FRAME {
        RadioFrame::Small(tx_bytes.to_vec())
    } else {
        RadioFrame::TxRef { hash, sender }
    }
}
