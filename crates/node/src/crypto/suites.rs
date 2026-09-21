//! The node's side of ADR-007: suite ids, addresses, and the activation gate.
//!
//! The registry, the envelope and the migration engine live in
//! `maya_crypto_pq`; this module is the thin layer that decides *when* the
//! chain accepts them. Until [`SUITE_ENVELOPE_ACTIVATION_HEIGHT`] every
//! suite-tagged transaction is refused, so the v5/v6 hybrid format stays the
//! only one any block can contain.
//!
//! [`SUITE_ENVELOPE_ACTIVATION_HEIGHT`]: crate::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT

use maya_crypto_pq::agility::{Network, SuitePolicy};
use maya_crypto_pq::suite::SuiteId;
use maya_governance::params::ParameterKey;

use crate::error::{NodeError, Result};
use crate::governance::ParameterTable;
use crate::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT;

/// Domain for the address of a suite-tagged key. Distinct from the hybrid
/// address domain, so a v7 key can never collide with a v5 account.
const SUITE_ADDRESS_DOMAIN: &str = "maya2c 2026-09-21 suite-tagged account address v1";

/// The address a suite-tagged key controls: `BLAKE3-derive-key(suite ‖ pk)`.
///
/// The suite byte is inside the hash, so the same bytes read as a key of
/// another suite name a different account.
#[must_use]
pub fn suite_address(suite: SuiteId, public_key: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(SUITE_ADDRESS_DOMAIN);
    hasher.update(&[suite.to_byte()]);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// The default suite governance has chosen.
///
/// # Errors
///
/// As [`suite_for_default`].
pub fn default_suite(parameters: &ParameterTable) -> Result<SuiteId> {
    suite_for_default(parameters.get(ParameterKey::DefaultSignatureSuite))
}

/// The suite a `DefaultSignatureSuite` value names, if mainnet permits it.
///
/// The governance crate can only range-check the byte (it has no
/// dependencies, invariant 12), and the range `0x10..=0x30` contains bytes
/// that name no suite. This is the registry half of the check, run at both
/// moments invariant 12 checks — proposal and execution — by
/// [`check_parameter`], and again whenever the table is read.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] for a value naming no suite, or a suite
/// mainnet does not permit.
pub fn suite_for_default(raw: u64) -> Result<SuiteId> {
    let byte = u8::try_from(raw)
        .map_err(|_| NodeError::SignatureSuite(format!("default suite {raw} is not a byte")))?;
    let suite = SuiteId::try_from(byte).map_err(|e| NodeError::SignatureSuite(e.to_string()))?;
    if SuitePolicy::genesis(Network::Mainnet).permitted(suite) {
        Ok(suite)
    } else {
        Err(NodeError::SignatureSuite(format!(
            "{suite:?} may not be the mainnet default"
        )))
    }
}

/// The node-side half of a parameter change's validation: governance's own
/// range check first, then this for the keys whose meaning the governance
/// crate cannot know.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] for a `DefaultSignatureSuite` value that
/// names no permitted suite.
pub fn check_parameter(key: ParameterKey, value: u64) -> Result<()> {
    match key {
        ParameterKey::DefaultSignatureSuite => suite_for_default(value).map(|_| ()),
        _ => Ok(()),
    }
}

/// The policy in force: the genesis schedule with governance's default.
///
/// # Errors
///
/// As [`default_suite`].
pub fn policy(parameters: &ParameterTable) -> Result<SuitePolicy> {
    SuitePolicy::genesis(Network::Mainnet)
        .with_default(default_suite(parameters)?)
        .map_err(|e| NodeError::SignatureSuite(e.to_string()))
}

/// Whether a suite-tagged transaction may be verified at all at `height`.
///
/// # Errors
///
/// [`NodeError::SignatureSuite`] before activation, or when `policy` does not
/// let `suite` sign at `height`.
pub fn check_admissible(policy: &SuitePolicy, suite: SuiteId, height: u64) -> Result<()> {
    if height < SUITE_ENVELOPE_ACTIVATION_HEIGHT {
        return Err(NodeError::SignatureSuite(format!(
            "suite-tagged transactions are not active at height {height}"
        )));
    }
    if !policy.may_sign(suite, height) {
        return Err(NodeError::SignatureSuite(format!(
            "{suite:?} may not sign at height {height}"
        )));
    }
    Ok(())
}
