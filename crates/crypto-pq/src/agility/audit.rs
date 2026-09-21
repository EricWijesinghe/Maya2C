//! The security-level audit.
//!
//! Run against a policy, it reports every suite that is below
//! [`MIN_PQ_SECURITY_BITS`](super::MIN_PQ_SECURITY_BITS) and still able to
//! sign — on testnet and mainnet [`SuitePolicy::permitted`] already refuses
//! those, so in practice this fires on devnet, which is where Ed25519 lives —
//! and every deprecated suite that accounts still need to leave. Findings are
//! data for an operator or a governance proposal; the audit changes nothing.

use super::{MIN_PQ_SECURITY_BITS, SuitePolicy, SuiteStatus};
use crate::suite::{SuiteId, SuiteInfo};

/// Why a suite was flagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    /// Claimed post-quantum security below the floor, and not yet sunset.
    BelowSecurityFloor {
        /// The suite's claimed bits.
        pq_bits: u16,
    },
    /// Deprecated; accounts still on it need to migrate.
    Deprecated {
        /// First height it will refuse to sign.
        sunset_height: u64,
    },
}

/// One audit finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    /// The suite.
    pub suite: SuiteId,
    /// Why.
    pub kind: FindingKind,
}

/// Audits every suite in the registry against `policy` at `height`.
#[must_use]
pub fn audit(policy: &SuitePolicy, height: u64) -> Vec<Finding> {
    SuiteId::ALL
        .into_iter()
        .flat_map(|suite| findings_for(policy, suite.info(), height))
        .collect()
}

fn findings_for(policy: &SuitePolicy, info: &SuiteInfo, height: u64) -> Vec<Finding> {
    let status = policy.status(info.id, height);
    let live = matches!(status, SuiteStatus::Active | SuiteStatus::Deprecated { .. });
    let mut out = Vec::new();

    if live && info.pq_security_bits < MIN_PQ_SECURITY_BITS {
        out.push(Finding {
            suite: info.id,
            kind: FindingKind::BelowSecurityFloor {
                pq_bits: info.pq_security_bits,
            },
        });
    }
    if let SuiteStatus::Deprecated { sunset_height } = status {
        out.push(Finding {
            suite: info.id,
            kind: FindingKind::Deprecated { sunset_height },
        });
    }
    out
}
