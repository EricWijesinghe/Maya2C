//! Remote validator signer (Master Prompt 16 §1-2).
//!
//! Validator consensus keys live here, not on the validator host. The node
//! asks over [`channel`]; the [`service`] checks every request against
//! [`protection`] — which refuses anything that could be slashed, whatever the
//! node says — and only then signs with a [`backend`].
//!
//! Backends: the encrypted [`keystore`] is built; PKCS#11 HSMs and cloud KMS
//! answer [`backend::BackendError::Unavailable`] with the reason, because no
//! integration was built or tested here (ADR-022).

pub mod backend;
pub mod channel;
pub mod keystore;
pub mod protection;
pub mod service;
