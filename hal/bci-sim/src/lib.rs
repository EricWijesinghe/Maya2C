//! **SIM.** EEG / brain-computer-interface authentication, as the research
//! simulator Master Prompt 6 §7 asks for: an EEG feature parser, a
//! commitment, proofs that expire after three seconds, and replay and spoof
//! rejection.
//!
//! Every recording this crate authenticates in its tests comes from
//! [`sim`], a synthetic generator, and says so in its EDF header. Nothing
//! here has been run on a human EEG, and nothing establishes that EEG
//! features are stable or distinctive enough to authenticate people; that is
//! an open research question, not an assumption this crate may make.
//!
//! What is REAL:
//! - [`edf`], a parser for the European Data Format that clinical EEG uses;
//! - the pipeline: spectral features are binarised into a 2,048-bit
//!   template, which feeds `maya-personhood`'s fuzzy commitment. That gives
//!   an ML-DSA-65 key, challenge-bound proofs, a 3 s expiry and replay
//!   rejection.
//!
//! Spoofing a live user with a recording is countered by a stimulus: the
//! challenge names a flicker frequency (steady-state visually evoked
//! potential, SSVEP), and a live occipital signal shows it. A replayed
//! recording shows the frequency of the session it was made in. The check
//! runs on the device, so **a device that skips it defeats it**: it means
//! something only inside attested hardware, which does not exist here
//! (invariant 11).

// Signal processing on sample counts and frequencies that stay far below
// 2^52; `usize` to `f64` loses nothing here.
#![allow(clippy::cast_precision_loss)]

pub mod edf;
pub mod features;
pub mod sim;

use maya_personhood::{Challenge, Commitment, Helper, PROOF_BYTES, Secret, Template};
use rand_core::CryptoRngCore;

/// Declared in logs and output wherever this crate's data appears.
pub const SIM: &str = "SIM: synthetic EEG (maya-bci-sim), not a human recording";

/// Why a step was refused.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum BciError {
    /// The EDF file is malformed.
    #[error("malformed EDF: {0}")]
    Malformed(&'static str),
    /// The recording lacks what the pipeline needs.
    #[error("unusable recording: {0}")]
    Unusable(&'static str),
    /// The recording does not show the challenged stimulus: a replay, or a
    /// user not looking.
    #[error("no response at the challenged {0} Hz stimulus")]
    NotLive(f64),
    /// From the fuzzy commitment and the signature.
    #[error(transparent)]
    Personhood(#[from] maya_personhood::PersonhoodError),
}

/// The stimulus frequency a challenge implies, from its nonce, so the
/// verifier and the device agree without another field.
#[must_use]
pub fn stimulus_hz(challenge: &Challenge) -> f64 {
    features::STIMULI[usize::from(challenge.nonce[0]) % features::STIMULI.len()]
}

/// Enrolls on the device from a recording made without a stimulus.
///
/// # Errors
///
/// A malformed or unusable recording.
pub fn enroll(edf_bytes: &[u8], secret: &Secret) -> Result<(Helper, Commitment), BciError> {
    let template: Template = features::template(&edf::parse(edf_bytes)?)?;
    Ok(maya_personhood::enroll(&template, secret))
}

/// On the device: answers `challenge` from a fresh recording, which must
/// show the challenge's stimulus.
///
/// # Errors
///
/// [`BciError::NotLive`] for a recording without the stimulus, or the
/// personhood errors (another person, a failed signature).
pub fn respond(
    helper: &Helper,
    edf_bytes: &[u8],
    challenge: &Challenge,
    rng: &mut impl CryptoRngCore,
) -> Result<[u8; PROOF_BYTES], BciError> {
    let recording = edf::parse(edf_bytes)?;
    let hz = stimulus_hz(challenge);
    if !features::responds_to(&recording, hz)? {
        return Err(BciError::NotLive(hz));
    }
    let template = features::template(&recording)?;
    Ok(maya_personhood::prove(helper, &template, challenge, rng)?)
}
