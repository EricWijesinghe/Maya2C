//! The rejoin guard (B49): which trust a catch-up may use, at start-up and
//! while following.
//!
//! A node whose committed state is older than the unbonding window cannot tell
//! the real chain from a long-range forgery by its own state: the validators
//! that signed it have unbonded, so their old keys sign a chain at no cost. Such
//! a node catches up only through a trusted block (`--trusted-block`), which the
//! chain has to pass through, and that block has to be recent. This is node
//! policy. Block validity and state roots are unchanged. The wall clock is read
//! here and passed into the pure functions, never read inside consensus, and the
//! decisions are pure so their boundaries test without a devnet.

use std::error::Error;
use std::sync::{Arc, Mutex};

use custom_l1_node::consensus::bft::catchup::Anchor;
use custom_l1_node::consensus::{Chain, TARGET_BLOCK_TIME};
use custom_l1_node::rpc::bootstrap::RpcBootstrapSource;
use custom_l1_node::state_pruner::snapshot::TrustedBlock;

use super::lock_chain;

/// Slack for a tip stamped a little ahead of this host's clock. Hosts run NTP,
/// and two minutes covers ordinary drift. A tip further ahead than that counts
/// as stale: a clock that cannot be trusted fails closed.
pub(crate) const CLOCK_SKEW_SECS: u64 = 120;

/// Why a node counts as stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stale {
    /// A source's claimed newest epoch is more than the window past this node's
    /// committed epoch.
    Epochs {
        local: u64,
        claimed: u64,
        window: u64,
    },
    /// This node's tip is older than the window of wall-clock time.
    Time { age_secs: u64, window_secs: u64 },
    /// This node's tip is later than this host's clock, beyond the allowance.
    FutureTip { ahead_secs: u64 },
}

/// The committed staking facts the guard reads from this node's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Local {
    /// The staking epoch the state is in.
    pub(crate) epoch: u64,
    /// `staking.params.unbonding_epochs`.
    pub(crate) unbonding_epochs: u64,
    /// Blocks per staking epoch, from genesis.
    pub(crate) epoch_blocks: u64,
    /// Unix seconds of this node's tip header.
    pub(crate) tip_timestamp: u64,
}

/// Unix seconds now. A clock that cannot be read returns `u64::MAX`, which makes
/// every tip look ancient, so the guard fails closed rather than open.
pub(crate) fn clock_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(u64::MAX, |elapsed| elapsed.as_secs())
}

/// The unbonding window in seconds. The block time is the protocol's intended
/// rate (`TARGET_BLOCK_TIME`), not a measured one, so this is an estimate. A
/// chain that runs faster or slower moves only the time trigger.
#[must_use]
pub(crate) fn window_secs(unbonding_epochs: u64, epoch_blocks: u64) -> u64 {
    unbonding_epochs
        .saturating_mul(epoch_blocks)
        .saturating_mul(TARGET_BLOCK_TIME)
}

/// Whether `local` is stale.
///
/// `claimed` is the newest epoch a source reports. It is UNVERIFIED: nothing
/// checks it against a quorum this node trusts. It is used only to tighten the
/// guard, so a false high claim forces a trusted block (fail-safe), and a low or
/// missing claim leaves only the time trigger.
///
/// Boundaries are strict: exactly one window behind or old is not stale; one
/// more is.
#[must_use]
pub(crate) fn stale(local: Local, claimed: Option<u64>, now: u64) -> Option<Stale> {
    let window = local.unbonding_epochs;
    if let Some(claimed) = claimed.filter(|c| c.saturating_sub(local.epoch) > window) {
        return Some(Stale::Epochs {
            local: local.epoch,
            claimed,
            window,
        });
    }
    // Fail closed: a tip later than the clock is not age zero, it is stale.
    if local.tip_timestamp > now.saturating_add(CLOCK_SKEW_SECS) {
        return Some(Stale::FutureTip {
            ahead_secs: local.tip_timestamp - now,
        });
    }
    let window_secs = window_secs(local.unbonding_epochs, local.epoch_blocks);
    let age_secs = now.saturating_sub(local.tip_timestamp);
    (age_secs > window_secs).then_some(Stale::Time {
        age_secs,
        window_secs,
    })
}

/// Whether `trusted` is recent enough to anchor on: the staking epoch it sits in
/// is within the window of the claimed head. Further back, the committee that
/// signed it has unbonded, and so has any committee the linked blocks wrote in
/// after it.
#[must_use]
pub(crate) fn is_recent(
    trusted: TrustedBlock,
    claimed: u64,
    window: u64,
    epoch_blocks: u64,
) -> bool {
    let trusted_epoch = trusted.height / epoch_blocks.max(1);
    claimed.saturating_sub(trusted_epoch) <= window
}

/// A trusted block at or below the local tip must be this node's block at that
/// height. Otherwise the node holds another chain, or the block is not one it
/// can check, and it must not start.
///
/// # Errors
///
/// The refusal text, when the trusted height is at or below `tip_height` and
/// `local_id_at` is not the trusted id.
pub(crate) fn check_against_tip(
    trusted: Option<TrustedBlock>,
    tip_height: u64,
    local_id_at: Option<[u8; 32]>,
) -> Result<(), String> {
    let Some(block) = trusted.filter(|t| t.height <= tip_height) else {
        return Ok(());
    };
    if local_id_at == Some(block.id) {
        return Ok(());
    }
    Err(format!(
        "the trusted block at height {} is not this node's block at that height (its tip is {tip_height}): the node holds another chain, or the block is not above the tip. Take a trusted block above this node's tip, or from the chain this node already follows",
        block.height
    ))
}

/// The anchor a catch-up uses, from the pure facts.
///
/// A stale node with no trusted block is refused, with the way out in the
/// message. A stale node whose trusted block is too old for the network's head
/// is refused too: its committee has already unbonded.
///
/// # Errors
///
/// The refusal text.
pub(crate) fn decide(
    stale: Option<Stale>,
    trusted: Option<TrustedBlock>,
    epoch_blocks: u64,
) -> Result<Anchor, String> {
    match (stale, trusted) {
        (None, None) => Ok(Anchor::Unanchored),
        (None, Some(block)) => Ok(Anchor::Checked(block)),
        (Some(reason), None) => Err(refusal(reason)),
        (
            Some(Stale::Epochs {
                claimed, window, ..
            }),
            Some(block),
        ) if !is_recent(block, claimed, window, epoch_blocks) => {
            Err(too_old(block, claimed, window, epoch_blocks))
        }
        (Some(_), Some(block)) => Ok(Anchor::Stale(block)),
    }
}

fn refusal(reason: Stale) -> String {
    let why = match reason {
        Stale::Epochs {
            local,
            claimed,
            window,
        } => format!(
            "a source claims the newest checkpoint is in epoch {claimed}, more than the {window}-epoch unbonding window past this node's epoch {local}"
        ),
        Stale::Time {
            age_secs,
            window_secs,
        } => format!(
            "this node's last block is {age_secs} s old, past the {window_secs} s unbonding window"
        ),
        Stale::FutureTip { ahead_secs } => format!(
            "this node's last block is {ahead_secs} s ahead of this host's clock: the clock is wrong, or the block is not what it claims to be"
        ),
    };
    format!(
        "this node is stale: {why}. Validators that unbonded since can sign a chain that never happened (a long-range attack), and this node's own state cannot tell it apart. Pass --trusted-block HEIGHT:HEX with a RECENT block (within the unbonding window of the network's head) above this node's tip, taken from the explorer, a peer you trust, or the published checkpoint, then restart with --catch-up-from"
    )
}

fn too_old(block: TrustedBlock, claimed: u64, window: u64, epoch_blocks: u64) -> String {
    let epoch = block.height / epoch_blocks.max(1);
    format!(
        "the trusted block at height {} is in epoch {epoch}, more than the {window}-epoch unbonding window behind the network's head (epoch {claimed}): the committee that signed it may have unbonded. Pass a RECENT trusted block, within the window of the head",
        block.height
    )
}

/// Refuses a stale node whose trusted block was never reached: no source
/// answered, or none served a chain through it. The pure half of
/// [`require_reached`].
///
/// # Errors
///
/// The refusal text, when the anchor is `Stale` and `reached` is false.
pub(crate) fn reached_or_err(anchor: Anchor, reached: bool) -> Result<(), String> {
    match anchor {
        Anchor::Stale(block) if !reached => Err(format!(
            "this stale node did not reach its trusted block at height {}: no source answered, or none served a chain through it. Give a trusted block that a reachable peer holds, above this node's tip, and restart",
            block.height
        )),
        _ => Ok(()),
    }
}

/// Whether the local chain passes through `trusted`: held at its height with
/// the trusted id.
#[must_use]
pub(crate) fn passes_through(chain: &Chain, trusted: TrustedBlock) -> bool {
    trusted.height <= chain.height() && block_id_at(chain, trusted.height) == Some(trusted.id)
}

/// The id of this node's block at `height`, walked down from the tip. `None`
/// above the tip or on a broken index.
#[must_use]
pub(crate) fn block_id_at(chain: &Chain, height: u64) -> Option<[u8; 32]> {
    let mut id = chain.tip();
    loop {
        let record = chain.get(&id)?;
        if record.height == height {
            return Some(id);
        }
        if record.height < height {
            return None;
        }
        id = record.parent?;
    }
}

/// The start-up half of the check, for a catch-up that has finished: a stale
/// node must have reached its trusted block.
///
/// # Errors
///
/// As [`reached_or_err`].
pub(crate) fn require_reached(chain: &Chain, anchor: Anchor) -> Result<(), String> {
    match anchor {
        Anchor::Stale(block) => reached_or_err(anchor, passes_through(chain, block)),
        _ => Ok(()),
    }
}

/// Whether a follower has its trusted block, tracked once per round. Returns
/// `Ok(true)` once the trusted block is held (or none was given), and refuses a
/// trusted block at or below the tip that this node does not hold.
///
/// # Errors
///
/// As [`check_against_tip`].
pub(crate) fn track_trusted(
    chain: &Chain,
    trusted: Option<TrustedBlock>,
    reached: bool,
) -> Result<bool, String> {
    let Some(block) = trusted else {
        return Ok(true);
    };
    if reached {
        return Ok(true);
    }
    if block.height > chain.height() {
        return Ok(false);
    }
    check_against_tip(
        Some(block),
        chain.height(),
        block_id_at(chain, block.height),
    )?;
    Ok(true)
}

/// The anchor for one follow round, from the pure facts. Time trigger only: a
/// follower has no claimed head without a probe. `pending` is the trusted block
/// not yet reached, which must lie above the tip.
///
/// # Errors
///
/// The refusal from [`decide`]. A running follower that is stale with nothing
/// pending stops, and the node exits so it can be restarted with a trusted block.
pub(crate) fn round_decision(
    local: Option<Local>,
    pending: Option<TrustedBlock>,
    now: u64,
) -> Result<Anchor, String> {
    let Some(local) = local else {
        return Ok(pending.map_or(Anchor::Unanchored, Anchor::Checked));
    };
    decide(stale(local, None, now), pending, local.epoch_blocks)
}

/// The committed staking facts, or `None` on a chain without staking, where
/// there is no unbonding window to measure.
fn local_of(chain: &Chain) -> Result<Option<Local>, Box<dyn Error>> {
    Ok(chain.state().committed_staking()?.map(|record| Local {
        epoch: record.staking.epoch,
        unbonding_epochs: record.staking.params.unbonding_epochs,
        epoch_blocks: record.epoch_blocks,
        tip_timestamp: chain.get(&chain.tip()).map_or(0, |b| b.header.timestamp),
    }))
}

/// The anchor for a follow round, read from the chain now.
///
/// # Errors
///
/// A state read that fails, or the refusal from [`round_decision`].
pub(crate) fn round_anchor(
    chain: &Chain,
    pending: Option<TrustedBlock>,
) -> Result<Anchor, Box<dyn Error>> {
    Ok(round_decision(local_of(chain)?, pending, clock_now())?)
}

/// The anchor for this start-up catch-up. Reads the committed state, probes the
/// sources for their claimed newest epoch, and decides.
///
/// This runs with or without `--catch-up-from`. A node started on the P2P sync
/// path is guarded too: a stale node with no trusted block is refused here.
///
/// # Errors
///
/// The refusal from [`decide`] or [`check_against_tip`], or a state read that
/// fails (the guard fails closed rather than guess).
pub(crate) async fn anchor(
    urls: &[String],
    chain: &Arc<Mutex<Chain>>,
    trusted: Option<TrustedBlock>,
) -> Result<Anchor, Box<dyn Error>> {
    let (local, tip_height, local_id) = {
        let c = lock_chain(chain);
        (
            local_of(&c)?,
            c.height(),
            trusted.and_then(|t| block_id_at(&c, t.height)),
        )
    };
    check_against_tip(trusted, tip_height, local_id)?;
    let Some(local) = local else {
        return Ok(trusted.map_or(Anchor::Unanchored, Anchor::Checked));
    };
    let claimed = newest_claimed_epoch(urls).await;
    let reason = stale(local, claimed, clock_now());
    if let Some(reason) = reason {
        println!("rejoin:      {reason:?}; catching up only through a recent trusted block");
    }
    Ok(decide(reason, trusted, local.epoch_blocks)?)
}

/// The highest epoch any answering source claims as its newest checkpoint, or
/// `None` when none answered.
///
/// UNVERIFIED. A peer's claim is checked against nothing, and it only tightens
/// the guard. A hostile source can force the trusted-block requirement, which is
/// fail-safe, but it can never lift it. If every source is hostile, only the
/// time trigger remains.
async fn newest_claimed_epoch(urls: &[String]) -> Option<u64> {
    let handle = tokio::runtime::Handle::current();
    let mut newest: Option<u64> = None;
    for url in urls {
        let source = match RpcBootstrapSource::new(url, handle.clone()) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("rejoin: probe of {url} failed: {e}");
                continue;
            }
        };
        // Blocking RPC, off the async workers, as in `bft::catch_up_once`.
        match tokio::task::spawn_blocking(move || source.probe_checkpoint()).await {
            Ok(Ok(Some(checkpoint))) => {
                newest = Some(newest.map_or(checkpoint.epoch, |e| e.max(checkpoint.epoch)));
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => eprintln!("rejoin: probe of {url} failed: {e}"),
            Err(e) => eprintln!("rejoin: probe of {url} failed: {e}"),
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
    fn a_claim_below_the_local_epoch_saturates_and_does_not_trip() {
        // Why: a claim behind this node's own epoch must not underflow into a trigger.
        assert_eq!(stale(LOCAL, Some(5), LOCAL.tip_timestamp), None);
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
    fn a_tip_in_the_future_counts_as_stale_beyond_the_skew_only() {
        // Why: a tip later than the clock is not "age zero"; past the allowance it
        // is stale, so a broken clock fails closed.
        let now = 1_000_000;
        let within = Local {
            tip_timestamp: now + CLOCK_SKEW_SECS,
            ..LOCAL
        };
        assert_eq!(stale(within, None, now), None);
        let beyond = Local {
            tip_timestamp: now + CLOCK_SKEW_SECS + 1,
            ..LOCAL
        };
        assert_eq!(
            stale(beyond, None, now),
            Some(Stale::FutureTip {
                ahead_secs: CLOCK_SKEW_SECS + 1
            })
        );
    }

    #[test]
    fn a_clock_that_cannot_be_read_makes_every_tip_stale() {
        // Why: the clock fallback is u64::MAX, which must read as ancient, not fresh.
        assert!(matches!(
            stale(LOCAL, None, u64::MAX),
            Some(Stale::Time { .. })
        ));
    }

    #[test]
    fn an_unanswered_probe_leaves_only_the_time_trigger() {
        assert_eq!(stale(LOCAL, None, LOCAL.tip_timestamp), None);
    }

    #[test]
    fn a_stale_node_without_a_trusted_block_is_refused_with_the_way_out() {
        let reason = stale(LOCAL, Some(18), LOCAL.tip_timestamp);
        let err = decide(reason, None, 100).unwrap_err();
        assert!(err.contains("--trusted-block HEIGHT:HEX"), "{err}");
        assert!(err.contains("RECENT"), "{err}");
    }

    #[test]
    fn a_stale_node_with_no_urls_is_refused_without_a_trusted_block() {
        // Why: with no --catch-up-from the time trigger alone must still refuse.
        let reason = stale(LOCAL, None, LOCAL.tip_timestamp + WINDOW_SECS + 1);
        assert!(decide(reason, None, 100).is_err());
    }

    #[test]
    fn a_stale_node_with_a_recent_trusted_block_is_anchored_and_a_fresh_one_is_checked() {
        // Height 1_100 is epoch 11: 7 behind head 18, so recent.
        let recent = TrustedBlock {
            height: 1_100,
            id: [7; 32],
        };
        let reason = stale(LOCAL, Some(18), LOCAL.tip_timestamp);
        assert_eq!(decide(reason, Some(recent), 100), Ok(Anchor::Stale(recent)));
        assert_eq!(
            decide(None, Some(TRUSTED), 100),
            Ok(Anchor::Checked(TRUSTED))
        );
        assert_eq!(decide(None, None, 100), Ok(Anchor::Unanchored));
    }

    #[test]
    fn a_trusted_block_older_than_the_window_behind_the_head_is_refused() {
        // Head epoch 18, window 7, 100 blocks per epoch: height 99 is epoch 0,
        // 18 behind (too old); height 1_100 is epoch 11, 7 behind (recent).
        let reason = stale(LOCAL, Some(18), LOCAL.tip_timestamp);
        let old = TrustedBlock {
            height: 99,
            id: [7; 32],
        };
        assert!(decide(reason, Some(old), 100).is_err());
        let recent = TrustedBlock {
            height: 1_100,
            id: [7; 32],
        };
        assert_eq!(decide(reason, Some(recent), 100), Ok(Anchor::Stale(recent)));
    }

    #[test]
    fn the_recency_boundary_is_exactly_one_window() {
        // Epoch 11 is 7 behind head 18 (recent); epoch 10 is 8 behind (too old).
        let at_edge = TrustedBlock {
            height: 1_100,
            id: [0; 32],
        };
        let one_past = TrustedBlock {
            height: 1_000,
            id: [0; 32],
        };
        assert!(is_recent(at_edge, 18, 7, 100));
        assert!(!is_recent(one_past, 18, 7, 100));
    }

    #[test]
    fn a_trusted_block_at_or_below_the_tip_must_be_the_local_block() {
        let held = Some(TrustedBlock {
            height: 50,
            id: [9; 32],
        });
        // A mismatching local id is refused.
        assert!(check_against_tip(held, 60, Some([1; 32])).is_err());
        // A missing local block at that height is refused.
        assert!(check_against_tip(held, 60, None).is_err());
        // The matching local block passes.
        assert!(check_against_tip(held, 60, Some([9; 32])).is_ok());
        // Above the tip nothing is checked here: it is checked as it is reached.
        assert!(check_against_tip(held, 40, None).is_ok());
        assert!(check_against_tip(None, 40, None).is_ok());
    }

    #[test]
    fn a_stale_node_that_reached_no_trusted_block_fails_hard() {
        let stale_anchor = Anchor::Stale(TRUSTED);
        assert!(reached_or_err(stale_anchor, false).is_err());
        assert!(reached_or_err(stale_anchor, true).is_ok());
        assert!(reached_or_err(Anchor::Checked(TRUSTED), false).is_ok());
        assert!(reached_or_err(Anchor::Unanchored, false).is_ok());
    }

    #[test]
    fn a_stale_follower_with_nothing_pending_stops_and_one_with_a_trusted_block_follows() {
        // Why: the follow loop re-checks staleness each round. A node that goes
        // stale while running must stop, unless it still has a block to reach.
        let stale_now = LOCAL.tip_timestamp + WINDOW_SECS + 1;
        assert!(round_decision(Some(LOCAL), None, stale_now).is_err());
        assert_eq!(
            round_decision(Some(LOCAL), Some(TRUSTED), stale_now),
            Ok(Anchor::Stale(TRUSTED))
        );
        assert_eq!(
            round_decision(Some(LOCAL), None, LOCAL.tip_timestamp),
            Ok(Anchor::Unanchored)
        );
        // A chain without staking has no window to measure.
        assert_eq!(
            round_decision(None, None, stale_now),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn the_window_saturates_rather_than_wrapping() {
        assert_eq!(window_secs(u64::MAX, 2), u64::MAX);
        assert_eq!(window_secs(7, 100), WINDOW_SECS);
    }
}
