//! The node's side of ADR-007: suite ids, addresses, and the activation gate.
//!
//! The registry, the envelope and the migration engine live in
//! `maya_crypto_pq`; this module is the thin layer that decides *when* the
//! chain accepts them. [`SUITE_ENVELOPE_ACTIVATION_HEIGHT`] is genesis
//! (ADR-013), and every verification path judges under
//! [`verification_policy`], a constant, so no vote and no config file can
//! change which signatures a block's validity rests on.
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
pub use maya_crypto_pq::suite::SUITE_ADDRESS_DOMAIN;

/// The address a suite-tagged key controls: `BLAKE3-derive-key(suite ‖ pk)`.
///
/// The suite byte is inside the hash, so the same bytes read as a key of
/// another suite name a different account.
#[must_use]
pub fn suite_address(suite: SuiteId, public_key: &[u8]) -> [u8; 32] {
    // One implementation, in `crypto-pq`, so apps that do not link the node
    // (chat, wallet) derive the same address.
    maya_crypto_pq::suite::suite_address(suite, public_key)
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

/// The policy consensus verifies under: the genesis schedule, mainnet rules,
/// at every height and on every network.
///
/// Deliberately *not* a function of state or of configuration. Three reasons,
/// and each one is a fork this avoids:
///
/// - [`SuitePolicy::may_sign`] reads only the deprecation schedule and the
///   height, never the default, so the governance table cannot change whether a
///   signature verifies. A verification rule that moved with a vote would make
///   the validity of a *historical* block depend on a later one.
/// - The network is fixed at `Mainnet` rather than read from config, because
///   [`SuitePolicy::permitted`] is laxer on `Devnet` (it admits `0x01`, which
///   has no post-quantum security at all). A node's own config file must never
///   decide which signatures its chain accepts.
/// - It allocates nothing and reads no column family, so the apply path spends
///   no I/O on it per transaction.
///
/// The consequence is worth stating: Ed25519 (`0x01`) can never sign a
/// suite-tagged frame on any network. It stays in the registry for tooling and
/// for the parity tests, and the audit in [`crate::crypto::suites`] flags it at
/// zero post-quantum bits.
#[must_use]
pub fn verification_policy() -> SuitePolicy {
    SuitePolicy::genesis(Network::Mainnet)
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
// The activation comparison is always false while the height is 0 (ADR-013),
// and clippy denies a comparison it can prove constant. It stays anyway: the
// constant is the one place activation is decided, and a later ADR that raised
// it — to re-darken the envelope on a fork, say — must take effect without
// anybody remembering to restore a check that had been deleted as dead code.
#[allow(clippy::absurd_extreme_comparisons)]
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
