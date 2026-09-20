//! The node's answer to `host_verify_zkml_proof`.
//!
//! `maya-vm` defines the question and charges for it; `maya-zkml` knows how to
//! check a proof; this is the ten lines that join them, and the one policy
//! decision that belongs to neither: **what a mismatched model id means.**
//!
//! A contract names the model it expects and supplies a verifying key. If the
//! key is not that model, the proof — however valid for the key it came with —
//! says nothing about the model the contract asked about. That is
//! [`ZkmlVerdict::Invalid`], a `0` the contract can act on, and not a trap:
//! whoever supplied the key may be the transaction's sender rather than the
//! contract's author, and a trap would hand them a way to abort the block.

use maya_vm::zkml::{ZKML_MODEL_ID_LEN, ZkmlVerdict};
use maya_zkml::ZkmlError;

/// Checks the key names the model, then checks the proof.
#[must_use]
pub fn verdict(
    model_id: &[u8; ZKML_MODEL_ID_LEN],
    vk: &[u8],
    public: &[i64],
    proof: &[u8],
) -> ZkmlVerdict {
    if maya_zkml::verify::model_id(vk) != *model_id {
        return ZkmlVerdict::Invalid;
    }
    match maya_zkml::verify::verify(vk, public, proof) {
        Ok(true) => ZkmlVerdict::Valid,
        Ok(false) => ZkmlVerdict::Invalid,
        Err(error @ (ZkmlError::MalformedKey(_) | ZkmlError::Oversized { .. })) => {
            ZkmlVerdict::Malformed(error.to_string())
        }
        // Every other variant belongs to the prover or the importer and cannot
        // come out of `verify`. If one ever did, "malformed" is the answer that
        // stops the call rather than guessing at a boolean.
        Err(other) => ZkmlVerdict::Malformed(other.to_string()),
    }
}

/// Refuses to start a value-bearing chain with zkML active on an untrusted SRS.
///
/// A no-op while `activation` is `u64::MAX`, which it is everywhere today. It
/// exists so that the day somebody sets a height, the node refuses mainnet
/// until `maya_zkml::srs::SRS_IS_TRUSTED` is also true — rather than that being
/// a second change somebody has to remember. `bins/maya2c-node/src/main.rs` calls it at
/// startup, beside the Groth16 setup check.
///
/// # Errors
///
/// [`ZkmlError::UntrustedSetup`] for a value-bearing chain with zkML reachable.
pub fn check_setup(chain_id: &str, activation: u64) -> Result<(), ZkmlError> {
    if activation == u64::MAX {
        return Ok(());
    }
    maya_zkml::srs::check_chain(chain_id)
}
