//! Submitting a threshold-encrypted transaction.
//!
//! # What the gateway does and does not do
//!
//! It validates *shape* — hex, length, non-empty — and forwards. It does
//! **not** decrypt, and it holds no share of the committee key. That is the
//! point of a threshold-encrypted mempool: the party that orders transactions
//! cannot read them, and a gateway that could decrypt would reintroduce
//! exactly the observer the scheme exists to remove.
//!
//! So the ciphertext is opaque here, and every check below is a check a proxy
//! can make without understanding the payload.
//!
//! # Why there are size limits at all
//!
//! The gateway terminates public HTTP. Without a ceiling, a submission
//! endpoint is a way to make the process allocate whatever the sender feels
//! like. [`MAX_SEALED_PAYLOAD_BYTES`] is checked **before** the hex is decoded,
//! so a hostile length never becomes a hostile allocation — the discipline
//! `core/codec.rs` applies on the wire, applied at the edge.

use serde::{Deserialize, Serialize};

use crate::error::GatewayError;

/// Largest sealed payload the gateway will forward, in bytes of ciphertext.
///
/// # Where the number comes from
///
/// A Maya2C transaction carries a hybrid signature pair of 11,165 bytes before
/// any payload, and the threshold encryption adds its own header. 64 KiB leaves
/// room for a transaction several times larger than any the chain currently
/// produces, while keeping a single request's allocation bounded by something
/// far below what a connection costs.
///
/// It is a gateway policy, not a consensus rule. The node applies its own
/// limits and is not relieved of them by this one.
pub const MAX_SEALED_PAYLOAD_BYTES: usize = 64 * 1024;

/// A sealed transaction submission.
#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct SealedSubmission {
    /// Threshold-encrypted transaction, hex-encoded, without a `0x` prefix.
    pub ciphertext: String,
}

/// What the gateway returns for an accepted submission.
#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct SealedAccepted {
    /// Hex digest the caller can use to correlate the submission.
    pub id: String,
    /// Ciphertext length in bytes, echoed so a caller can confirm framing.
    pub bytes: usize,
}

/// Validates a submission's shape and returns the decoded ciphertext.
///
/// # Errors
///
/// - [`GatewayError::PayloadTooLarge`] if the hex is longer than
///   [`MAX_SEALED_PAYLOAD_BYTES`] would allow, checked before decoding.
/// - [`GatewayError::BadRequest`] for empty input, odd length, or a non-hex
///   character.
pub fn validate(submission: &SealedSubmission) -> Result<Vec<u8>, GatewayError> {
    let hex_len = submission.ciphertext.len();

    // Checked before `hex::decode` allocates. Two hex characters per byte, so
    // the ceiling on the string is twice the ceiling on the payload.
    if hex_len > MAX_SEALED_PAYLOAD_BYTES * 2 {
        return Err(GatewayError::PayloadTooLarge(format!(
            "sealed payload of {} hex characters exceeds the {} byte ceiling",
            hex_len, MAX_SEALED_PAYLOAD_BYTES
        )));
    }

    if hex_len == 0 {
        return Err(GatewayError::BadRequest(
            "sealed payload is empty".to_string(),
        ));
    }

    if !hex_len.is_multiple_of(2) {
        return Err(GatewayError::BadRequest(
            "sealed payload has an odd number of hex characters".to_string(),
        ));
    }

    hex::decode(&submission.ciphertext)
        .map_err(|_| GatewayError::BadRequest("sealed payload is not valid hex".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submission(ciphertext: &str) -> SealedSubmission {
        SealedSubmission {
            ciphertext: ciphertext.to_string(),
        }
    }

    #[test]
    fn a_well_formed_payload_decodes() {
        let decoded = validate(&submission("deadbeef")).expect("valid hex");
        assert_eq!(decoded, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn an_empty_payload_is_refused() {
        assert!(matches!(
            validate(&submission("")),
            Err(GatewayError::BadRequest(_))
        ));
    }

    #[test]
    fn an_odd_length_payload_is_refused() {
        assert!(matches!(
            validate(&submission("abc")),
            Err(GatewayError::BadRequest(_))
        ));
    }

    #[test]
    fn a_non_hex_payload_is_refused() {
        assert!(matches!(
            validate(&submission("zzzz")),
            Err(GatewayError::BadRequest(_))
        ));
    }

    #[test]
    fn an_oversized_payload_is_refused_before_it_is_decoded() {
        // The ceiling is checked against the *string*, so this never allocates
        // the 64 KiB it describes. A `hex::decode` first would have.
        let oversized = "ab".repeat(MAX_SEALED_PAYLOAD_BYTES + 1);
        assert!(matches!(
            validate(&submission(&oversized)),
            Err(GatewayError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn a_payload_exactly_at_the_ceiling_is_accepted() {
        let at_limit = "ab".repeat(MAX_SEALED_PAYLOAD_BYTES);
        let decoded = validate(&submission(&at_limit)).expect("at the ceiling");
        assert_eq!(decoded.len(), MAX_SEALED_PAYLOAD_BYTES);
    }

    #[test]
    fn the_size_check_precedes_the_hex_check() {
        // An oversized payload that is *also* invalid hex must report the size.
        // Two gateways disagreeing about which rule fired would disagree in any
        // rate limiter or ban score keyed on the error.
        let oversized_garbage = "zz".repeat(MAX_SEALED_PAYLOAD_BYTES + 1);
        assert!(matches!(
            validate(&submission(&oversized_garbage)),
            Err(GatewayError::PayloadTooLarge(_))
        ));
    }
}
