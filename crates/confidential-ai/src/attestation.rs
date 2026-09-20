//! Hardware attestation: a hook, not the root of trust.
//!
//! # Why a hook
//!
//! An SGX DCAP quote chains to Intel's root key and an SEV-SNP report to AMD's,
//! both ECDSA. On this chain that makes attestation a statement by a vendor —
//! a trusted party, which is invariant 11's territory — signed with a scheme a
//! quantum attacker forges, about hardware with a record of side-channel
//! breaks. So the privacy of an update here never depends on it: that comes
//! from masking and noise ([`crate::protocol`], [`crate::dp`]). Attestation can
//! add one claim on top — that the aggregation ran as a measured binary — and
//! only if a verifier for real reports is supplied.
//!
//! # What is not here
//!
//! Report generation needs SGX or SEV-SNP hardware; the development host has
//! neither. Report verification needs the vendor certificate chains and
//! recorded reports to test against, and none are in the tree. The crates that
//! would do it — `sev` (AMD SEV-SNP) and `dcap-qvl` (Intel DCAP) — are not
//! dependencies. Until one is, every report is [`Verdict::Unattested`], and
//! [`require_attested`] refuses it.

use crate::error::{Error, Result};

/// A verifier's conclusion about one report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The report is genuine and binds this round's transcript.
    Attested {
        /// The measurement of the code that produced it.
        measurement: [u8; 32],
    },
    /// Nothing can be concluded. Carries why.
    Unattested(&'static str),
}

/// Checks a hardware report against the transcript it must bind.
pub trait AttestationVerifier {
    /// The verdict on `report` for a round whose transcript is `transcript`.
    fn verify(&self, report: &[u8], transcript: &[u8; 32]) -> Verdict;
}

/// The verifier in force until a real one is supplied.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAttestation;

impl AttestationVerifier for NoAttestation {
    fn verify(&self, _report: &[u8], _transcript: &[u8; 32]) -> Verdict {
        Verdict::Unattested("no attestation verifier is configured")
    }
}

/// The measurement from an attested verdict.
///
/// # Errors
///
/// [`Error::Attestation`] for anything unattested — never treated as a pass.
pub fn require_attested(verdict: &Verdict) -> Result<[u8; 32]> {
    match verdict {
        Verdict::Attested { measurement } => Ok(*measurement),
        Verdict::Unattested(reason) => Err(Error::Attestation(reason)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// A stand-in that accepts a report equal to the transcript: exercises the
    /// binding, verifies no hardware.
    struct EchoVerifier;

    impl AttestationVerifier for EchoVerifier {
        fn verify(&self, report: &[u8], transcript: &[u8; 32]) -> Verdict {
            if report == transcript {
                Verdict::Attested {
                    measurement: [7; 32],
                }
            } else {
                Verdict::Unattested("report does not bind this transcript")
            }
        }
    }

    #[test]
    fn without_a_verifier_nothing_is_attested() {
        let verdict = NoAttestation.verify(b"anything", &[0; 32]);
        assert!(require_attested(&verdict).is_err());
    }

    #[test]
    fn a_report_for_another_transcript_is_refused() {
        let transcript = [1; 32];
        assert_eq!(
            require_attested(&EchoVerifier.verify(&transcript, &transcript)),
            Ok([7; 32])
        );
        assert!(require_attested(&EchoVerifier.verify(&[2; 32], &transcript)).is_err());
    }
}
