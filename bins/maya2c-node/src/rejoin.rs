//! The rejoin guard (B49): which trust a catch-up may use, at start-up and
//! while following.
//!
//! A node whose committed state is older than the unbonding window cannot tell
//! the real chain from a long-range forgery by its own state: the validators
//! that signed it have unbonded, so their old keys sign a chain at no cost. Such
//! a node catches up only through a trusted block (`--trusted-block`), which the
//! chain has to pass through, and that block has to be recent.
//!
//! Staleness needs a CONFIRMATION. A node's own clock or tip never refuses by
//! itself: a genesis-only node whose genesis is old, or a halted network, looks
//! identical to a forgery from the node's side, so a time-old tip counts only
//! when a source reports a head beyond the window. The claimed head is
//! unverified. It only ever tightens the guard, and only the maximum claim over
//! the answering sources is used.
//!
//! This is node policy. Block validity and state roots are unchanged. The wall
//! clock is read here and passed into the pure functions, never read inside
//! consensus, and the decisions are pure so their boundaries test without a devnet.

use std::error::Error;
use std::sync::{Arc, Mutex};

use custom_l1_node::consensus::bft::catchup::Anchor;
use custom_l1_node::consensus::{Chain, TARGET_BLOCK_TIME};
use custom_l1_node::rpc::bootstrap::RpcBootstrapSource;
use custom_l1_node::state_pruner::snapshot::TrustedBlock;

use super::lock_chain;

/// Slack for a tip stamped a little ahead of this host's clock. Hosts run NTP,
/// and two minutes covers ordinary drift. A tip further ahead is a warning, not
/// a refusal: it cannot make a node stale by itself.
pub(crate) const CLOCK_SKEW_SECS: u64 = 120;

/// How often a running follower whose tip is old re-probes its source. Why: a
/// halted network leaves every follower old, and a probe every second would
/// load the source for nothing. Thirty rounds is thirty seconds.
pub(crate) const FOLLOW_PROBE_EVERY: u32 = 30;

/// The newest head any answering source reports, per field. UNVERIFIED: a
/// claim is checked against nothing this node trusts. It is the maximum over
/// the sources, so one honest source reporting the head keeps the guard strict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Claimed {
    /// The highest checkpoint epoch any source reports.
    pub(crate) epoch: u64,
    /// The highest checkpoint height any source reports.
    pub(crate) height: u64,
}

impl Claimed {
    /// The per-field maximum of two claims.
    #[must_use]
    pub(crate) fn join(self, other: Claimed) -> Claimed {
        Claimed {
            epoch: self.epoch.max(other.epoch),
            height: self.height.max(other.height),
        }
    }
}

/// Why a node counts as stale. Both carry the claimed head epoch, which the
/// recency rule measures against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stale {
    /// The claimed head epoch is more than the window past this node's epoch.
    Epochs {
        local: u64,
        claimed: u64,
        window: u64,
    },
    /// This node's tip is older than the window of time, and a source reports a
    /// head more than the window of blocks past the tip.
    Time {
        age_secs: u64,
        window_secs: u64,
        claimed: u64,
    },
}

/// The committed staking facts the guard reads from this node's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Local {
    /// This node's tip height.
    pub(crate) height: u64,
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
/// every tip look old, so the time trigger then needs a confirming head, and no
/// refusal is made on the clock alone.
pub(crate) fn clock_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(u64::MAX, |elapsed| elapsed.as_secs())
}

/// The unbonding window in seconds. The block time is the protocol's intended
/// rate (`TARGET_BLOCK_TIME`), not a measured one, so this is an estimate.
#[must_use]
pub(crate) fn window_secs(unbonding_epochs: u64, epoch_blocks: u64) -> u64 {
    unbonding_epochs
        .saturating_mul(epoch_blocks)
        .saturating_mul(TARGET_BLOCK_TIME)
}

/// Whether this node's tip is older than the window of time.
#[must_use]
pub(crate) fn time_old(local: Local, now: u64) -> bool {
    now.saturating_sub(local.tip_timestamp)
        > window_secs(local.unbonding_epochs, local.epoch_blocks)
}

/// Whether `local` is stale.
///
/// Two triggers, both needing `claimed` (no source answered means not stale):
/// the claimed head epoch is more than the window past this node's epoch; or
/// the tip is older than the window of time AND the claimed head is more than
/// the window of blocks past the tip. A time-old tip with no head beyond the
/// window is not stale: that is a halted network or a genesis-only node.
///
/// Boundaries are strict: exactly one window is not stale; one more is.
#[must_use]
pub(crate) fn stale(local: Local, claimed: Option<Claimed>, now: u64) -> Option<Stale> {
    let head = claimed?;
    let window = local.unbonding_epochs;
    if head.epoch.saturating_sub(local.epoch) > window {
        return Some(Stale::Epochs {
            local: local.epoch,
            claimed: head.epoch,
            window,
        });
    }
    let window_secs = window_secs(local.unbonding_epochs, local.epoch_blocks);
    let window_blocks = window.saturating_mul(local.epoch_blocks);
    let age_secs = now.saturating_sub(local.tip_timestamp);
    (age_secs > window_secs && head.height.saturating_sub(local.height) > window_blocks).then_some(
        Stale::Time {
            age_secs,
            window_secs,
            claimed: head.epoch,
        },
    )
}

/// The warning for a time-old tip with no confirming head: its age, when the
/// node would otherwise be refused on time alone. `None` when it is not old,
/// or when a head confirms it (then `stale` decides).
#[must_use]
pub(crate) fn unconfirmed_age(local: Local, claimed: Option<Claimed>, now: u64) -> Option<u64> {
    if !time_old(local, now) || stale(local, claimed, now).is_some() {
        return None;
    }
    Some(now.saturating_sub(local.tip_timestamp))
}

/// The tip's lead over this host's clock beyond the allowance, for a warning.
#[must_use]
pub(crate) fn future_ahead(local: Local, now: u64) -> Option<u64> {
    let ahead = local.tip_timestamp.checked_sub(now)?;
    (ahead > CLOCK_SKEW_SECS).then_some(ahead)
}

/// Whether `trusted` is recent enough to anchor on: the staking epoch it sits in
/// is within the window of the claimed head. `claimed` is the MAXIMUM claim, so a
/// lower claim from another source cannot loosen this. Further back, the
/// committee that signed the block has unbonded.
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
/// message. A stale node whose trusted block is not recent for the claimed head
/// is refused too: the committee that signed it has unbonded.
///
/// # Errors
///
/// The refusal text.
pub(crate) fn decide(
    local: Local,
    claimed: Option<Claimed>,
    now: u64,
    trusted: Option<TrustedBlock>,
) -> Result<Anchor, String> {
    match (stale(local, claimed, now), trusted) {
        (None, None) => Ok(Anchor::Unanchored),
        (None, Some(block)) => Ok(Anchor::Checked(block)),
        (Some(reason), None) => Err(refusal(reason)),
        (Some(reason), Some(block)) => {
            // Recency applies whatever the stale reason, against the claimed head.
            let head = match reason {
                Stale::Epochs { claimed, .. } | Stale::Time { claimed, .. } => claimed,
            };
            let window = local.unbonding_epochs;
            if !is_recent(block, head, window, local.epoch_blocks) {
                return Err(too_old(block, head, window, local.epoch_blocks));
            }
            Ok(Anchor::Stale(block))
        }
    }
}

fn refusal(reason: Stale) -> String {
    let why = match reason {
        Stale::Epochs {
            local,
            claimed,
            window,
        } => format!(
            "a source reports the head in epoch {claimed}, more than the {window}-epoch unbonding window past this node's epoch {local}"
        ),
        Stale::Time {
            age_secs,
            window_secs,
            claimed,
        } => format!(
            "this node's last block is {age_secs} s old, past the {window_secs} s unbonding window, and a source reports a head in epoch {claimed} beyond the window"
        ),
    };
    format!(
        "this node is stale: {why}. Validators that unbonded since can sign a chain that never happened (a long-range attack), and this node's own state cannot tell it apart. Pass --trusted-block HEIGHT:HEX with a RECENT block (within the unbonding window of the head) above this node's tip, taken from the explorer, a peer you trust, or the published checkpoint, then restart with --catch-up-from. If this node was offline longer than the window allows, restore from a snapshot with --bootstrap-from"
    )
}

fn too_old(block: TrustedBlock, claimed: u64, window: u64, epoch_blocks: u64) -> String {
    let epoch = block.height / epoch_blocks.max(1);
    format!(
        "the trusted block at height {} is in epoch {epoch}, more than the {window}-epoch unbonding window behind the head (epoch {claimed}, the maximum any source reports): the committee that signed it may have unbonded. Pass a RECENT trusted block, within the window of the head",
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

/// The start-up half of the check, for a catch-up that has finished: a stale node
/// must have reached its trusted block.
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

/// The anchor for one follow round, from the pure facts. `pending` is the trusted
/// block not yet reached, which must lie above the tip. `claimed` is the head a
/// probe reported this round, or `None` when the tip is fresh (the probe is only
/// made for an old tip).
///
/// # Errors
///
/// The refusal from [`decide`]. A running follower that is stale with nothing
/// pending stops, and the node exits so it can be restarted with a trusted block.
pub(crate) fn round_decision(
    local: Option<Local>,
    pending: Option<TrustedBlock>,
    claimed: Option<Claimed>,
    now: u64,
) -> Result<Anchor, String> {
    let Some(local) = local else {
        return Ok(pending.map_or(Anchor::Unanchored, Anchor::Checked));
    };
    decide(local, claimed, now, pending)
}

/// The committed staking facts, or `None` on a chain without staking, where
/// there is no unbonding window to measure.
pub(crate) fn local_state(chain: &Chain) -> Result<Option<Local>, Box<dyn Error>> {
    Ok(chain.state().committed_staking()?.map(|record| Local {
        height: chain.height(),
        epoch: record.staking.epoch,
        unbonding_epochs: record.staking.params.unbonding_epochs,
        epoch_blocks: record.epoch_blocks,
        tip_timestamp: chain.get(&chain.tip()).map_or(0, |b| b.header.timestamp),
    }))
}

/// The anchor for a follow round, read from the chain now and the probe.
///
/// # Errors
///
/// The refusal from [`round_decision`].
pub(crate) fn round_anchor(
    chain: &Chain,
    pending: Option<TrustedBlock>,
    claimed: Option<Claimed>,
) -> Result<Anchor, Box<dyn Error>> {
    let local = local_state(chain)?;
    Ok(round_decision(local, pending, claimed, clock_now())?)
}

/// The anchor for this start-up catch-up. Reads the committed state, probes the
/// sources for their claimed head, and decides.
///
/// This runs with or without `--catch-up-from`. A node started on the P2P sync
/// path is guarded too. A time-old tip with no confirming head is a warning and
/// the node starts: a genesis-only node, or a halted network, must not be refused.
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
            local_state(&c)?,
            c.height(),
            trusted.and_then(|t| block_id_at(&c, t.height)),
        )
    };
    check_against_tip(trusted, tip_height, local_id)?;
    let Some(local) = local else {
        return Ok(trusted.map_or(Anchor::Unanchored, Anchor::Checked));
    };
    let claimed = newest_claimed(urls).await;
    let now = clock_now();
    if let Some(age) = unconfirmed_age(local, claimed, now) {
        eprintln!(
            "rejoin: WARNING: this node's tip is {age} s old and no source reports a head beyond the unbonding window. Starting unconfirmed; a halted network looks like this. If it is a forgery, only a trusted block helps, and it is required once a head is reported beyond the window"
        );
    }
    if let Some(ahead) = future_ahead(local, now) {
        eprintln!(
            "rejoin: WARNING: this node's tip is {ahead} s ahead of this host's clock. The clock may be wrong"
        );
    }
    let reason = stale(local, claimed, now);
    if let Some(reason) = reason {
        println!("rejoin:      {reason:?}; catching up only through a recent trusted block");
    }
    Ok(decide(local, claimed, now, trusted)?)
}

/// The maximum head any answering source reports, or `None` when none answered.
pub(crate) async fn newest_claimed(urls: &[String]) -> Option<Claimed> {
    let mut newest: Option<Claimed> = None;
    for url in urls {
        if let Some(claim) = probe(url).await {
            newest = Some(newest.map_or(claim, |n| n.join(claim)));
        }
    }
    newest
}

/// One source's claimed head, or `None` when it did not answer. Errors are
/// printed, never dropped. UNVERIFIED: see [`Claimed`].
pub(crate) async fn probe(url: &str) -> Option<Claimed> {
    let source = match RpcBootstrapSource::new(url, tokio::runtime::Handle::current()) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("rejoin: probe of {url} failed: {e}");
            return None;
        }
    };
    // Blocking RPC, off the async workers, as in `bft::catch_up_once`.
    match tokio::task::spawn_blocking(move || source.probe_checkpoint()).await {
        Ok(Ok(Some(checkpoint))) => Some(Claimed {
            epoch: checkpoint.epoch,
            height: checkpoint.height,
        }),
        Ok(Ok(None)) => None,
        Ok(Err(e)) => {
            eprintln!("rejoin: probe of {url} failed: {e}");
            None
        }
        Err(e) => {
            eprintln!("rejoin: probe of {url} failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// Epoch 10 at height 1000, window 7 epochs of 100 blocks = 700 blocks, or
    /// 10,500 s at 15 s. Tip at t=1e6.
    const LOCAL: Local = Local {
        height: 1_000,
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
    /// A recent trusted block: epoch 11, 7 behind a head of epoch 18.
    const RECENT: TrustedBlock = TrustedBlock {
        height: 1_100,
        id: [7; 32],
    };

    fn head(epoch: u64, height: u64) -> Option<Claimed> {
        Some(Claimed { epoch, height })
    }

    /// Time past the window, and the head far beyond it in blocks.
    fn old_now() -> u64 {
        LOCAL.tip_timestamp + WINDOW_SECS + 1
    }

    #[test]
    fn exactly_one_window_of_epochs_behind_is_not_stale() {
        assert_eq!(stale(LOCAL, head(17, 1_000), LOCAL.tip_timestamp), None);
    }

    #[test]
    fn one_epoch_past_the_window_is_stale_even_with_a_fresh_tip() {
        assert_eq!(
            stale(LOCAL, head(18, 1_000), LOCAL.tip_timestamp),
            Some(Stale::Epochs {
                local: 10,
                claimed: 18,
                window: 7
            })
        );
    }

    #[test]
    fn a_claim_below_the_local_epoch_saturates_and_does_not_trip() {
        assert_eq!(stale(LOCAL, head(5, 500), LOCAL.tip_timestamp), None);
    }

    #[test]
    fn a_fresh_genesis_with_an_old_timestamp_and_no_source_starts() {
        // Why: the regression. Height 0, genesis older than the window, nobody to
        // confirm it. The time trigger alone must never refuse.
        let genesis = Local {
            height: 0,
            epoch: 0,
            tip_timestamp: 1,
            ..LOCAL
        };
        assert_eq!(stale(genesis, None, u64::MAX), None);
        assert_eq!(
            decide(genesis, None, u64::MAX, None),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn a_time_old_tip_with_no_newer_head_starts_and_warns() {
        // Why: a halted network reports no head beyond the tip. The node starts,
        // and the warning is the only effect.
        let now = old_now();
        assert_eq!(stale(LOCAL, head(10, 1_000), now), None);
        assert_eq!(
            decide(LOCAL, head(10, 1_000), now, None),
            Ok(Anchor::Unanchored)
        );
        assert_eq!(
            unconfirmed_age(LOCAL, head(10, 1_000), now),
            Some(WINDOW_SECS + 1)
        );
        assert_eq!(unconfirmed_age(LOCAL, None, now), Some(WINDOW_SECS + 1));
    }

    #[test]
    fn a_time_old_tip_with_a_head_beyond_the_window_is_refused_without_a_trusted_block() {
        // Head 701 blocks past the tip, the window is 700.
        let now = old_now();
        let ahead = head(10, LOCAL.height + 701);
        assert!(matches!(stale(LOCAL, ahead, now), Some(Stale::Time { .. })));
        let err = decide(LOCAL, ahead, now, None).unwrap_err();
        assert!(err.contains("--trusted-block HEIGHT:HEX"), "{err}");
        assert!(err.contains("RECENT"), "{err}");
        assert_eq!(unconfirmed_age(LOCAL, ahead, now), None);
    }

    #[test]
    fn a_head_exactly_one_window_of_blocks_ahead_does_not_confirm() {
        let now = old_now();
        assert_eq!(stale(LOCAL, head(10, LOCAL.height + 700), now), None);
    }

    #[test]
    fn a_fresh_tip_is_never_stale_on_time_alone() {
        let ahead = head(10, LOCAL.height + 10_000);
        assert_eq!(stale(LOCAL, ahead, LOCAL.tip_timestamp), None);
    }

    #[test]
    fn with_no_urls_there_is_no_claim_and_so_no_refusal() {
        // With no --catch-up-from there are no sources to probe. No claim means
        // no confirmation, so an old tip starts with a warning.
        assert_eq!(stale(LOCAL, None, old_now()), None);
        assert_eq!(decide(LOCAL, None, old_now(), None), Ok(Anchor::Unanchored));
    }

    #[test]
    fn a_tip_in_the_future_is_a_warning_not_a_refusal() {
        let now = 1_000_000;
        let beyond = Local {
            tip_timestamp: now + CLOCK_SKEW_SECS + 1,
            ..LOCAL
        };
        assert_eq!(future_ahead(beyond, now), Some(CLOCK_SKEW_SECS + 1));
        assert_eq!(stale(beyond, None, now), None);
        assert_eq!(decide(beyond, None, now, None), Ok(Anchor::Unanchored));
        let within = Local {
            tip_timestamp: now + CLOCK_SKEW_SECS,
            ..LOCAL
        };
        assert_eq!(future_ahead(within, now), None);
    }

    #[test]
    fn a_stale_node_with_a_recent_trusted_block_is_anchored_and_a_fresh_one_is_checked() {
        let reason_claim = head(18, 1_000);
        assert_eq!(
            decide(LOCAL, reason_claim, LOCAL.tip_timestamp, Some(RECENT)),
            Ok(Anchor::Stale(RECENT))
        );
        assert_eq!(
            decide(LOCAL, None, LOCAL.tip_timestamp, Some(TRUSTED)),
            Ok(Anchor::Checked(TRUSTED))
        );
        assert_eq!(
            decide(LOCAL, None, LOCAL.tip_timestamp, None),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn a_trusted_block_older_than_the_window_behind_the_head_is_refused() {
        // Head epoch 18: height 99 is epoch 0 (too old), height 1_100 is epoch 11 (recent).
        let claim = head(18, 1_000);
        assert!(decide(LOCAL, claim, LOCAL.tip_timestamp, Some(TRUSTED)).is_err());
        assert_eq!(
            decide(LOCAL, claim, LOCAL.tip_timestamp, Some(RECENT)),
            Ok(Anchor::Stale(RECENT))
        );
    }

    #[test]
    fn recency_applies_to_a_time_stale_node_too() {
        // Why: once a time-old tip is confirmed by a head, the trusted block must
        // be recent against that head, whatever the stale reason.
        let now = old_now();
        let claim = head(18, LOCAL.height + 701);
        assert!(matches!(stale(LOCAL, claim, now), Some(Stale::Time { .. })));
        assert!(decide(LOCAL, claim, now, Some(TRUSTED)).is_err());
        assert_eq!(
            decide(LOCAL, claim, now, Some(RECENT)),
            Ok(Anchor::Stale(RECENT))
        );
    }

    #[test]
    fn recency_is_checked_against_the_maximum_claim() {
        // Why: one source says epoch 11, another says epoch 18. The maximum (18)
        // is what the trusted block must be recent for. RECENT is 7 behind 18, so
        // it passes; a block at epoch 10 passes against 11 but not against 18.
        let lower = Claimed {
            epoch: 11,
            height: 1_200,
        };
        let higher = Claimed {
            epoch: 18,
            height: 1_900,
        };
        let joined = lower.join(higher);
        assert_eq!(joined.epoch, 18);
        let at_epoch_10 = TrustedBlock {
            height: 1_000,
            id: [7; 32],
        };
        // The lower claim alone does not make the node stale, so nothing is recency-checked.
        assert_eq!(
            decide(LOCAL, Some(lower), LOCAL.tip_timestamp, Some(at_epoch_10)),
            Ok(Anchor::Checked(at_epoch_10))
        );
        // The maximum does: epoch 10 is 8 behind epoch 18.
        assert!(decide(LOCAL, Some(joined), LOCAL.tip_timestamp, Some(at_epoch_10)).is_err());
        assert_eq!(
            decide(LOCAL, Some(joined), LOCAL.tip_timestamp, Some(RECENT)),
            Ok(Anchor::Stale(RECENT))
        );
    }

    #[test]
    fn a_trusted_block_at_or_below_the_tip_must_be_the_local_block() {
        let held = Some(TrustedBlock {
            height: 50,
            id: [9; 32],
        });
        assert!(check_against_tip(held, 60, Some([1; 32])).is_err());
        assert!(check_against_tip(held, 60, None).is_err());
        assert!(check_against_tip(held, 60, Some([9; 32])).is_ok());
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
    fn a_follower_exits_only_when_a_newer_head_beyond_the_window_is_confirmed() {
        // Why: a halt leaves no newer head, so the follower keeps running. Only a
        // confirmed head beyond the window, with nothing pending, stops it.
        let now = old_now();
        let confirmed = head(10, LOCAL.height + 701);
        assert!(round_decision(Some(LOCAL), None, confirmed, now).is_err());
        assert_eq!(
            round_decision(Some(LOCAL), None, None, now),
            Ok(Anchor::Unanchored)
        );
        assert_eq!(
            round_decision(Some(LOCAL), None, head(10, LOCAL.height + 10), now),
            Ok(Anchor::Unanchored)
        );
        assert_eq!(
            round_decision(Some(LOCAL), Some(RECENT), confirmed, now),
            Ok(Anchor::Stale(RECENT))
        );
        assert_eq!(
            round_decision(None, None, confirmed, now),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn the_window_saturates_rather_than_wrapping() {
        assert_eq!(window_secs(u64::MAX, 2), u64::MAX);
        assert_eq!(window_secs(7, 100), WINDOW_SECS);
    }
}
