//! `maya2c-audit`: recomputes Maya2C's supply and mining-reserve emission from a
//! node's public RPC and the genesis file, so that nobody has to take a node's
//! word that no coins were printed.
//!
//! This crate does not depend on the node. Arithmetic is `maya-amount` and
//! `maya-ledger-math`; the emission schedule is `maya-spec-ref`, written from
//! `spec/09-mining.md`. Each check reads only what the RPC serves, and says
//! NOT VERIFIED for anything the node does not keep.

pub mod emission;
pub mod fees;
pub mod genesis;
pub mod numbers;
pub mod observe;
pub mod report;
pub mod rpc;
pub mod supply;

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod scenario_tests;

use genesis::Plan;
use report::{Finding, Report};
use rpc::{Rpc, RpcError};

/// Why the audit could not run to a report. Each stops the run; none is a
/// finding about the chain.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error(transparent)]
    Genesis(#[from] genesis::GenesisError),
    #[error("window {from}..={to} is outside the chain (tip {tip})")]
    Window { from: u64, to: u64, tip: u64 },
    #[error("cannot read genesis file: {0}")]
    GenesisFile(String),
    #[error("cannot write report: {0}")]
    Output(String),
}

/// Runs every supply check over `from ..= to`. `None` bounds mean "from genesis"
/// and "to the tip".
///
/// # Errors
///
/// See [`AuditError`]: an unreadable node, or a window the chain does not have.
pub fn audit_supply(
    rpc: &dyn Rpc,
    plan: &Plan,
    from: Option<u64>,
    to: Option<u64>,
) -> Result<Report, AuditError> {
    let tip = rpc.chain_info()?.height;
    let from = from.unwrap_or(1);
    let to = to.unwrap_or(tip);
    if from == 0 || from > to || to > tip {
        return Err(AuditError::Window { from, to, tip });
    }
    let observed = observe::observe(rpc, from, to)?;
    let findings: Vec<Finding> = vec![
        supply::conservation(rpc, plan)?,
        supply::reserve_reconciles(rpc, plan, &observed)?,
        emission::check(rpc, plan, &observed)?,
        fees::check(plan, &observed),
    ];
    Ok(Report {
        tool: "maya2c-audit supply",
        tip,
        from,
        to,
        findings,
        not_verified_here: NOT_VERIFIED_HERE,
    })
}

/// What this tool cannot check from the RPC, printed with every report so a
/// signer sees the boundary of what the PASS lines cover.
pub const NOT_VERIFIED_HERE: &[&str] = &[
    "genesis id: get_chain_info.genesis is not recomputed (the id needs node code); compare it with the ceremony transcript",
    "accounts: get_supply sums the node's accounts; the tool does not enumerate them",
    "emission split within an epoch: committee part and miner shares are only bounded, per epoch",
    "base fee: burn is not compared with base fee x block bytes",
    "shielded pool: reported by get_supply, not asserted against an activation height",
];
