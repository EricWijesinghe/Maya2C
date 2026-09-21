//! The node's answer to `host_verify_zkml_proof`: there is no verifier.
//!
//! The halo2/KZG-over-BN254 verifier this module used to call was removed on
//! 2026-09-21 (ADR-008): a pairing-based proof is forgeable by a quantum
//! adversary, and its SRS was derived from a public seed, so anyone could
//! forge one today (invariant 22). zkML becomes PLANNED until it is
//! re-expressed as a transparent STARK.
//!
//! The host function stays in the VM's ABI — removing an import a deployed
//! module links against is a harder break than answering it — and it was
//! dark anyway: `ZKML_ACTIVATION_HEIGHT` is `u64::MAX`, so
//! `ContractHost::verify_zkml` returns before reaching [`verdict`]. If it is
//! ever reached, the answer is `Malformed`, which stops the call rather than
//! guessing at a boolean.

use maya_vm::zkml::{ZKML_MODEL_ID_LEN, ZkmlVerdict};

/// Why nothing can verify a zkML proof.
pub const NO_VERIFIER: &str =
    "no zkML verifier: the halo2/KZG one was removed (ADR-008); a STARK verifier is planned";

/// Always [`ZkmlVerdict::Malformed`]: there is no verifier to ask.
#[must_use]
pub fn verdict(
    _model_id: &[u8; ZKML_MODEL_ID_LEN],
    _vk: &[u8],
    _public: &[i64],
    _proof: &[u8],
) -> ZkmlVerdict {
    ZkmlVerdict::Malformed(NO_VERIFIER.to_string())
}

/// Refused setup.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("zkML activation on {chain_id}: {NO_VERIFIER}")]
pub struct ZkmlUnavailable {
    /// The chain that tried.
    pub chain_id: String,
}

/// Refuses any chain that sets a zkML activation height: there is nothing
/// for it to activate.
///
/// # Errors
///
/// [`ZkmlUnavailable`] for any `activation` other than `u64::MAX`.
pub fn check_setup(chain_id: &str, activation: u64) -> Result<(), ZkmlUnavailable> {
    if activation == u64::MAX {
        Ok(())
    } else {
        Err(ZkmlUnavailable {
            chain_id: chain_id.to_owned(),
        })
    }
}
