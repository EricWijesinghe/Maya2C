//! `maya-validator-ledger`: a public record of validator participation, built
//! only from signed consensus data a public endpoint serves (invariant 14:
//! never from self-reported telemetry).
//!
//! What is public today is each recent epoch's quorum-attested checkpoint
//! (`get_checkpoint [epoch]`) with the committee indices that signed it, and
//! the serving node's own `get_bft_status`. Everything else a validator
//! ledger would want is listed in [`MISSING`] rather than estimated.

#![warn(missing_docs)]

use maya_invariant_monitor::public::{Checkpoint, parse_checkpoint};
use maya_sentinel::obs::Transport;
use serde_json::{Value, json};

/// What a full ledger needs and no public method serves. Printed with every
/// ledger so a reader never mistakes an absent column for a zero.
pub const MISSING: &[&str] = &[
    "stake: get_validators is not on the gateway allowlist, and no RPC serves the epoch-frozen weights the engine votes with (apps/status/src/decentral.rs)",
    "identity per committee index: committee public keys are not public, so signatures are counted, not verified, and an index is not a validator across epochs",
    "rounds per validator: get_certificates is not on the gateway allowlist; get_bft_status gives only the serving node's own round",
    "uptime: the signer column is checkpoint signatures, not liveness",
    "equivocation: no RPC serves evidence (crates/node/src/consensus/bft/evidence.rs)",
    "old epochs: get_checkpoint holds recent epochs only; an epoch it no longer holds shows as not held",
];

/// One epoch's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochRow {
    /// Staking epoch.
    pub epoch: u64,
    /// The newest checkpoint the endpoint holds for that epoch (for the
    /// current epoch it may not be the final one yet). `None` when not held.
    pub checkpoint: Option<Checkpoint>,
    /// Committee indices whose signature is not on that checkpoint. Known only
    /// for the current epoch (the one whose committee size the endpoint
    /// reports) and only when it has a checkpoint. Not a missed-round count.
    pub absent: Option<Vec<u16>>,
    /// Why the epoch could not be read, so a failed call is not shown as
    /// "not held".
    pub error: Option<String>,
}

/// The ledger one endpoint can give.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    /// Current staking epoch, per the endpoint.
    pub epoch: u64,
    /// Current committee size, per the endpoint.
    pub committee: u16,
    /// The serving node's last committed round.
    pub committed_round: u64,
    /// Rows, oldest first.
    pub rows: Vec<EpochRow>,
}

/// Reads the last `depth` epochs (including the current one) from `t`.
///
/// # Errors
///
/// Text naming the call when `get_bft_status` fails or answers `null` or an
/// unexpected shape (a node without DAG-BFT has no committee to record).
pub async fn build<T: Transport>(t: &T, depth: u64) -> Result<Ledger, String> {
    let bft = t
        .call("get_bft_status", vec![])
        .await
        .map_err(|e| format!("get_bft_status failed: {e}"))?;
    let field = |k: &str| bft.get(k).and_then(Value::as_u64);
    let (Some(epoch), Some(committee), Some(committed_round)) =
        (field("epoch"), field("committee"), field("committed_round"))
    else {
        return Err(format!(
            "get_bft_status answered without a committee: {bft}"
        ));
    };
    let committee = u16::try_from(committee)
        .map_err(|_| format!("get_bft_status committee {committee} is not a u16"))?;
    let first = epoch.saturating_sub(depth.saturating_sub(1));
    let mut rows = Vec::new();
    for e in first..=epoch {
        // One failed epoch (a 429, a timeout) is a row, not the end of the ledger.
        let (checkpoint, error) = match t.call("get_checkpoint", vec![e]).await {
            Ok(v) => (parse_checkpoint(&v), None),
            Err(err) => (None, Some(format!("get_checkpoint [{e}] failed: {err}"))),
        };
        let absent = checkpoint
            .as_ref()
            .filter(|_| e == epoch)
            .map(|c| (0..committee).filter(|i| !c.signers.contains(i)).collect());
        rows.push(EpochRow {
            epoch: e,
            checkpoint,
            absent,
            error,
        });
    }
    Ok(Ledger {
        epoch,
        committee,
        committed_round,
        rows,
    })
}

/// The ledger as text, with the MISSING list at the end.
#[must_use]
pub fn render_text(l: &Ledger) -> String {
    let mut out = format!(
        "validator ledger: epoch {}, committee {}, serving node committed round {}\n",
        l.epoch, l.committee, l.committed_round
    );
    for r in &l.rows {
        let line = match (&r.checkpoint, &r.absent) {
            (None, _) if r.error.is_some() => {
                format!(
                    "epoch {}: {}",
                    r.epoch,
                    r.error.as_deref().unwrap_or_default()
                )
            }
            (None, _) => format!("epoch {}: no checkpoint held by this endpoint", r.epoch),
            (Some(c), Some(absent)) => format!(
                "epoch {}: {}/{} signed, absent {absent:?} (height {}, signers {:?})",
                r.epoch,
                c.signers.len(),
                l.committee,
                c.height,
                c.signers
            ),
            (Some(c), None) => format!(
                "epoch {}: {} signed (height {}, signers {:?}; committee size not public)",
                r.epoch,
                c.signers.len(),
                c.height,
                c.signers
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    for m in MISSING {
        out.push_str("MISSING ");
        out.push_str(m);
        out.push('\n');
    }
    out
}

/// The ledger as JSON, with the MISSING list under `missing`.
#[must_use]
pub fn to_json(l: &Ledger) -> Value {
    let epochs: Vec<Value> = l
        .rows
        .iter()
        .map(|r| {
            json!({
                "epoch": r.epoch,
                "height": r.checkpoint.as_ref().map(|c| c.height),
                "block": r.checkpoint.as_ref().map(|c| c.block.clone()),
                "signers": r.checkpoint.as_ref().map(|c| c.signers.clone()),
                "absent": r.absent,
                "error": r.error,
            })
        })
        .collect();
    json!({
        "epoch": l.epoch,
        "committee": l.committee,
        "committed_round": l.committed_round,
        "epochs": epochs,
        "missing": MISSING,
    })
}
