//! The rejoin guard (B49): which trust a start-up catch-up may use.
//!
//! A node whose committed state is older than the unbonding window cannot tell
//! the real chain from a long-range forgery by its own state: the validators
//! that signed it have unbonded, so their old keys sign a chain at no cost. Such
//! a node catches up only through a trusted block (`--trusted-block`), which
//! the chain has to pass through. This is node policy. Block validity and state
//! roots are unchanged, and the wall clock is read here, never inside consensus.
//! The decision is pure, so its boundaries test without a devnet.

use std::error::Error;
use std::sync::{Arc, Mutex};

use custom_l1_node::consensus::bft::catchup::{Anchor, CheckpointSource};
use custom_l1_node::consensus::{Chain, TARGET_BLOCK_TIME};
use custom_l1_node::rpc::bootstrap::RpcBootstrapSource;
use custom_l1_node::state_pruner::snapshot::TrustedBlock;

use super::{lock_chain, unix_now};

/// Why a node counts as stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    /// A source's newest checkpoint is more than the window of epochs past
    /// this node's committed epoch.
    Epochs {
        local: u64,
        claimed: u64,
        window: u64,
    },
    /// This node's tip is older than the window of wall-clock time.
    Time { age_secs: u64, window_secs: u64 },
}

/// The committed staking facts the guard reads from this node's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    /// The staking epoch the state is in.
    pub epoch: u64,
    /// `staking.params.unbonding_epochs`.
    pub unbonding_epochs: u64,
    /// Blocks per staking epoch, from genesis.
    pub epoch_blocks: u64,
    /// Unix seconds of this node's tip header.
    pub tip_timestamp: u64,
}

/// The unbonding window in seconds. The block time is the protocol's intended
/// rate (`TARGET_BLOCK_TIME`), not a measured one, so this is an estimate. A
/// chain that runs faster or slower moves only the time trigger; the epoch
/// trigger does not depend on it.
#[must_use]
pub fn window_secs(unbonding_epochs: u64, epoch_blocks: u64) -> u64 {
    unbonding_epochs
        .saturating_mul(epoch_blocks)
        .saturating_mul(TARGET_BLOCK_TIME)
}

/// Whether `local` is stale. `claimed` is the newest epoch an answering source
/// reports (`None` when none answered, so only the time trigger can fire).
///
/// Why the boundaries are strict: exactly one window behind is still inside
/// what the committee can be expected to have kept; one more is outside it.
#[must_use]
pub fn stale(local: Local, claimed: Option<u64>, now: u64) -> Option<Stale> {
    let window = local.unbonding_epochs;
    if let Some(claimed) = claimed.filter(|c| c.saturating_sub(local.epoch) > window) {
        return Some(Stale::Epochs {
            local: local.epoch,
            claimed,
            window,
        });
    }
    let window_secs = window_secs(local.unbonding_epochs, local.epoch_blocks);
    let age_secs = now.saturating_sub(local.tip_timestamp);
    (age_secs > window_secs).then_some(Stale::Time {
        age_secs,
        window_secs,
    })
}

/// The anchor a catch-up uses. A stale node with no trusted block is refused,
/// with the way out in the message, mirroring `pruning::require_anchor`.
///
/// # Errors
///
/// The refusal text, when `stale` is set and `trusted` is not.
pub fn decide(stale: Option<Stale>, trusted: Option<TrustedBlock>) -> Result<Anchor, String> {
    match (stale, trusted) {
        (None, None) => Ok(Anchor::Unanchored),
        (None, Some(block)) => Ok(Anchor::Checked(block)),
        (Some(_), Some(block)) => Ok(Anchor::Stale(block)),
        (Some(reason), None) => Err(refusal(reason)),
    }
}

fn refusal(reason: Stale) -> String {
    let why = match reason {
        Stale::Epochs {
            local,
            claimed,
            window,
        } => format!(
            "the network's newest checkpoint is in epoch {claimed}, more than the {window}-epoch unbonding window past this node's epoch {local}"
        ),
        Stale::Time {
            age_secs,
            window_secs,
        } => format!(
            "this node's last block is {age_secs} s old, past the {window_secs} s unbonding window"
        ),
    };
    format!(
        "this node is stale: {why}. Validators that unbonded since can sign a chain that never happened (a long-range attack), and this node's own state cannot tell it apart. Pass --trusted-block HEIGHT:HEX, a block above this node's tip, taken from the explorer, a peer you trust, or the published checkpoint, then restart with --catch-up-from"
    )
}

/// The anchor for this start-up catch-up. Reads the committed state, asks the
/// sources for their newest checkpoint, and decides.
///
/// Without staking state there is no unbonding window to measure, so the guard
/// does not apply: a trusted block still binds.
///
/// # Errors
///
/// The refusal from [`decide`], or a state read that fails (the guard fails
/// closed rather than guess).
pub async fn anchor(
    urls: &[String],
    chain: &Arc<Mutex<Chain>>,
    trusted: Option<TrustedBlock>,
) -> Result<Anchor, Box<dyn Error>> {
    if urls.is_empty() {
        return Ok(Anchor::Unanchored);
    }
    let local = {
        let c = lock_chain(chain);
        c.state().committed_staking()?.map(|record| Local {
            epoch: record.staking.epoch,
            unbonding_epochs: record.staking.params.unbonding_epochs,
            epoch_blocks: record.epoch_blocks,
            tip_timestamp: c.get(&c.tip()).map_or(0, |b| b.header.timestamp),
        })
    };
    let Some(local) = local else {
        return Ok(trusted.map_or(Anchor::Unanchored, Anchor::Checked));
    };
    let reason = stale(local, newest_claimed_epoch(urls).await, unix_now());
    if let Some(reason) = reason {
        println!("rejoin:      {reason:?}; catching up only through the trusted block");
    }
    Ok(decide(reason, trusted)?)
}

/// The highest epoch any answering source claims as its newest checkpoint, or
/// `None` when none answered. The highest is used because one honest source is
/// enough to show the network has moved on; a source that overstates only makes
/// the guard stricter, never looser.
async fn newest_claimed_epoch(urls: &[String]) -> Option<u64> {
    let handle = tokio::runtime::Handle::current();
    let mut newest: Option<u64> = None;
    for url in urls {
        let Ok(source) = RpcBootstrapSource::new(url, handle.clone()) else {
            continue;
        };
        // Blocking RPC, off the async workers, as in `bft::catch_up_once`.
        let probed = tokio::task::spawn_blocking(move || source.checkpoint()).await;
        if let Ok(Ok(Some(checkpoint))) = probed {
            newest = Some(newest.map_or(checkpoint.epoch, |e| e.max(checkpoint.epoch)));
        }
    }
    newest
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// Epoch 10, window 7 epochs of 100 blocks at 15 s = 10,500 s, tip at t=1e6.
    const LOCAL: Local = Local {
        epoch: 10,
        unbonding_epochs: 7,
        epoch_blocks: 100,
        tip_timestamp: 1_000_000,
    };
    const WINDOW_SECS: u64 = 10_500;
    const TRUSTED: TrustedBlock = TrustedBlock {
        height: 99,
        id: [7; 32],
    };

    #[test]
    fn exactly_one_window_of_epochs_behind_is_not_stale() {
        // Why: the boundary itself is inside the window; only past it is a forgery risk.
        assert_eq!(stale(LOCAL, Some(17), LOCAL.tip_timestamp), None);
    }

    #[test]
    fn one_epoch_past_the_window_is_stale_even_with_a_fresh_tip() {
        // Why: the epoch trigger is independent of the tip's age.
        assert_eq!(
            stale(LOCAL, Some(18), LOCAL.tip_timestamp),
            Some(Stale::Epochs {
                local: 10,
                claimed: 18,
                window: 7
            })
        );
    }

    #[test]
    fn a_time_gap_of_exactly_the_window_is_not_stale_and_one_more_second_is() {
        let at_boundary = LOCAL.tip_timestamp + WINDOW_SECS;
        assert_eq!(stale(LOCAL, None, at_boundary), None);
        assert_eq!(
            stale(LOCAL, None, at_boundary + 1),
            Some(Stale::Time {
                age_secs: WINDOW_SECS + 1,
                window_secs: WINDOW_SECS
            })
        );
    }

    #[test]
    fn an_unanswered_probe_leaves_only_the_time_trigger() {
        assert_eq!(stale(LOCAL, None, LOCAL.tip_timestamp), None);
    }

    #[test]
    fn a_stale_node_without_a_trusted_block_is_refused_with_the_way_out() {
        let reason = stale(LOCAL, Some(18), LOCAL.tip_timestamp);
        let err = decide(reason, None).unwrap_err();
        assert!(err.contains("--trusted-block HEIGHT:HEX"), "{err}");
    }

    #[test]
    fn a_stale_node_with_a_trusted_block_is_anchored_and_a_fresh_one_is_checked() {
        let reason = stale(LOCAL, Some(18), LOCAL.tip_timestamp);
        assert_eq!(decide(reason, Some(TRUSTED)), Ok(Anchor::Stale(TRUSTED)));
        assert_eq!(decide(None, Some(TRUSTED)), Ok(Anchor::Checked(TRUSTED)));
        assert_eq!(decide(None, None), Ok(Anchor::Unanchored));
    }

    #[test]
    fn the_window_saturates_rather_than_wrapping() {
        assert_eq!(window_secs(u64::MAX, 2), u64::MAX);
        assert_eq!(window_secs(7, 100), WINDOW_SECS);
    }
}
