//! The rejoin guard (B49): which trust a catch-up may use, at start-up and
//! while following.
//!
//! A node whose committed state is older than the unbonding window cannot tell
//! the real chain from a long-range forgery by its own state: the validators
//! that signed it have unbonded, so their old keys sign a chain at no cost. Such
//! a node catches up only through a trusted block (`--trusted-block`), which the
//! chain has to pass through, and that block has to be recent.
//!
//! Trust model. A stale start is safe with `--trusted-block`. A time-old tip
//! with no confirming head is refused unless the operator passes
//! `--allow-unconfirmed-start`, because an eclipsed node (every source it
//! reaches is the attacker's) looks exactly like a halted network or an old
//! genesis from its own side. A running follower assumes at least one honest
//! source: it keeps following and warns when no head is confirmed. Heads
//! reported by sources are unverified. The maximum decides whether the
//! requirement applies and how recent the block must be.
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
    /// Seconds per block the window is measured at: observed from this node's
    /// own headers, capped at the target (see [`block_interval_secs`]).
    pub(crate) block_interval: u64,
}

/// Unix seconds now. A clock that cannot be read returns `u64::MAX`, which makes
/// every tip look old, so the time trigger then needs a confirming head, and an
/// unconfirmed start is refused unless the operator allows it.
pub(crate) fn clock_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(u64::MAX, |elapsed| elapsed.as_secs())
}

/// The seconds per block the window is measured at. Why the cap: this chain's
/// DAG-BFT blocks have been measured well under the 15 s target, and a slower
/// estimate would lengthen the window, so the time trigger would under-detect.
/// A shorter window is the strict direction; an honest late restart has
/// `--trusted-block` or the explicit flag.
///
/// `observed` is the mean over the last `epoch_blocks` blocks, from this node's
/// own headers: `back_ts` is the timestamp `epoch_blocks` below the tip. It is
/// used only when the delta is positive, and never above `TARGET_BLOCK_TIME`.
/// Without history, or with a non-positive delta, the target is used. Clamped to
/// at least one second.
#[must_use]
pub(crate) fn block_interval_secs(tip_ts: u64, back_ts: Option<u64>, epoch_blocks: u64) -> u64 {
    let observed = back_ts
        .and_then(|back| tip_ts.checked_sub(back))
        .filter(|delta| *delta > 0 && epoch_blocks > 0)
        .map(|delta| delta / epoch_blocks);
    observed
        .map_or(TARGET_BLOCK_TIME, |o| o.min(TARGET_BLOCK_TIME))
        .max(1)
}

/// The unbonding window in seconds, at the measured block interval.
#[must_use]
pub(crate) fn window_secs(unbonding_epochs: u64, epoch_blocks: u64, block_interval: u64) -> u64 {
    unbonding_epochs
        .saturating_mul(epoch_blocks)
        .saturating_mul(block_interval)
}

/// Whether this node's tip is older than the window of time.
#[must_use]
pub(crate) fn time_old(local: Local, now: u64) -> bool {
    now.saturating_sub(local.tip_timestamp) > window_of(local)
}

fn window_of(local: Local) -> u64 {
    window_secs(
        local.unbonding_epochs,
        local.epoch_blocks,
        local.block_interval,
    )
}

/// Whether `local` is stale.
///
/// Two triggers, both needing `claimed` (no source answered means not stale):
/// the claimed head epoch is more than the window past this node's epoch; or
/// the tip is older than the window of time AND the claimed head is more than
/// the window of blocks past the tip. A time-old tip with no head beyond the
/// window is not stale here. It is unconfirmed, and [`decide`] handles it.
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
    let window_secs = window_of(local);
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

/// The age of a time-old tip that no head confirms, or `None` when the tip is
/// fresh or stale is decided by a head.
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

/// Whether `trusted` is recent enough to anchor on: its staking epoch is at
/// least one epoch INSIDE the window of the claimed head, so a follower anchored
/// to it is not stale again mid-catch-up. `claimed` is the MAXIMUM claim, so a
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
    claimed.saturating_sub(trusted_epoch) < window
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
/// - Not stale, not time-old: [`Anchor::Unanchored`], or `Checked` with a trusted block.
/// - Stale (a head confirms it): needs a trusted block, recent for the claimed
///   head. It is refused otherwise.
/// - Time-old with no confirming head: a trusted block gives `Stale` (the
///   hash-linked path, no claim needed). With no trusted block it is refused,
///   unless `allow_unconfirmed` (the operator's `--allow-unconfirmed-start`).
///
/// # Errors
///
/// The refusal text.
pub(crate) fn decide(
    local: Local,
    claimed: Option<Claimed>,
    now: u64,
    trusted: Option<TrustedBlock>,
    allow_unconfirmed: bool,
) -> Result<Anchor, String> {
    let reason = stale(local, claimed, now);
    let unconfirmed = reason.is_none() && time_old(local, now);
    match (reason, trusted) {
        (None, _) if unconfirmed => match trusted {
            // A block at or below this node's own tip gives no catch-up: it is
            // the node's own history, so it does not lift the refusal.
            Some(block) if block.height > local.height => Ok(Anchor::Stale(block)),
            None if allow_unconfirmed => Ok(Anchor::Unanchored),
            _ => Err(unconfirmed_refusal(now.saturating_sub(local.tip_timestamp))),
        },
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
        "this node is stale: {why}. Validators that unbonded since can sign a chain that never happened (a long-range attack), and this node's own state cannot tell it apart. Pass --trusted-block HEIGHT:HEX with a RECENT block (at least one epoch inside the unbonding window of the head) above this node's tip, taken from the explorer, a peer you trust, or the published checkpoint, then restart with --catch-up-from. If this node was offline longer than the window allows, restore from a snapshot with --bootstrap-from"
    )
}

fn unconfirmed_refusal(age_secs: u64) -> String {
    format!(
        "this node's tip is {age_secs} s old and no source reports a head beyond the unbonding window, so nothing confirms the network moved on. There are two honest causes: the network halted for longer than the window, or this genesis was signed long before launch. The attack this refuses: an eclipsed node (every source it reaches is the attacker's) sees exactly the same thing, and the keys of validators that unbonded can sign a chain at no cost. Pass --trusted-block HEIGHT:HEX instead when possible (a block above the tip, at least one epoch inside the window, from two or more independent sources), or --bootstrap-from. Only if neither is possible, and you know which honest cause applies, pass --allow-unconfirmed-start"
    )
}

fn too_old(block: TrustedBlock, claimed: u64, window: u64, epoch_blocks: u64) -> String {
    let epoch = block.height / epoch_blocks.max(1);
    format!(
        "the trusted block at height {} is in epoch {epoch}, at or more than the {window}-epoch unbonding window behind the head (epoch {claimed}, the maximum any source reports): the committee that signed it may have unbonded. Pass a RECENT trusted block, at least one epoch inside the window of the head",
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
/// A running follower never refuses an unconfirmed (time-old, no head) tip. It
/// keeps following and warns, which is the residual documented in DECISIONS.
///
/// # Errors
///
/// The refusal from [`decide`] for a confirmed stale follower with nothing
/// pending. The node exits so it can be restarted with a trusted block.
pub(crate) fn round_decision(
    local: Option<Local>,
    pending: Option<TrustedBlock>,
    claimed: Option<Claimed>,
    now: u64,
) -> Result<Anchor, String> {
    let Some(local) = local else {
        return Ok(pending.map_or(Anchor::Unanchored, Anchor::Checked));
    };
    decide(local, claimed, now, pending, true)
}

/// The committed staking facts, or `None` on a chain without staking, where
/// there is no unbonding window to measure.
pub(crate) fn local_state(chain: &Chain) -> Result<Option<Local>, Box<dyn Error>> {
    let Some(record) = chain.state().committed_staking()? else {
        return Ok(None);
    };
    let tip_timestamp = chain.get(&chain.tip()).map_or(0, |b| b.header.timestamp);
    // One walk down the tip's history per call, `epoch_blocks` lookups.
    let back_ts = chain
        .height()
        .checked_sub(record.epoch_blocks)
        .and_then(|height| block_id_at(chain, height))
        .and_then(|id| chain.get(&id))
        .map(|b| b.header.timestamp);
    Ok(Some(Local {
        height: chain.height(),
        epoch: record.staking.epoch,
        unbonding_epochs: record.staking.params.unbonding_epochs,
        epoch_blocks: record.epoch_blocks,
        tip_timestamp,
        block_interval: block_interval_secs(tip_timestamp, back_ts, record.epoch_blocks),
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
/// Runs on every BFT chain, with or without `--catch-up-from`. A chain without
/// BFT is not covered. A time-old tip with no confirming head is refused unless
/// `allow_unconfirmed` is set (`--allow-unconfirmed-start`).
///
/// # Errors
///
/// The refusal from [`decide`] or [`check_against_tip`], or a state read that
/// fails (the guard fails closed rather than guess).
pub(crate) async fn anchor(
    urls: &[String],
    chain: &Arc<Mutex<Chain>>,
    trusted: Option<TrustedBlock>,
    allow_unconfirmed: bool,
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
    if let Some(ahead) = future_ahead(local, now) {
        eprintln!(
            "rejoin: WARNING: this node's tip is {ahead} s ahead of this host's clock. The clock may be wrong"
        );
    }
    if let Some(reason) = stale(local, claimed, now) {
        println!("rejoin:      {reason:?}; catching up only through a recent trusted block");
    }
    if allow_unconfirmed {
        if let Some(age) = unconfirmed_age(local, claimed, now) {
            eprintln!(
                "rejoin: WARNING: starting unconfirmed by operator choice (--allow-unconfirmed-start): the tip is {age} s old and no source reports a head beyond the window"
            );
        }
    }
    Ok(decide(local, claimed, now, trusted, allow_unconfirmed)?)
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
        block_interval: 15,
    };
    const WINDOW_SECS: u64 = 10_500;
    const TRUSTED: TrustedBlock = TrustedBlock {
        height: 99,
        id: [7; 32],
    };
    /// A recent trusted block: epoch 12, 6 behind a head of epoch 18, so one
    /// epoch inside the window.
    const RECENT: TrustedBlock = TrustedBlock {
        height: 1_200,
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
    fn a_fresh_genesis_inside_the_window_starts_with_no_flag() {
        // Why: the common case. Height 0, genesis inside the window, no source.
        let genesis = Local {
            height: 0,
            epoch: 0,
            tip_timestamp: 1_000_000 - 100,
            ..LOCAL
        };
        assert_eq!(
            decide(genesis, None, 1_000_000, None, false),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn a_fresh_genesis_with_an_old_timestamp_needs_the_flag() {
        // Why: height 0, genesis signed long before launch, no source. It is an
        // honest case, but it looks like an eclipse, so the operator says so.
        let genesis = Local {
            height: 0,
            epoch: 0,
            tip_timestamp: 1,
            ..LOCAL
        };
        let err = decide(genesis, None, old_now(), None, false).unwrap_err();
        assert!(err.contains("--allow-unconfirmed-start"), "{err}");
        assert!(err.contains("eclipse") || err.contains("eclipsed"), "{err}");
        assert_eq!(
            decide(genesis, None, old_now(), None, true),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn a_time_old_tip_with_no_claim_is_refused_without_the_flag_and_starts_with_it() {
        // Why: the eclipse guard. A halted network and an eclipse look the same.
        let now = old_now();
        let err = decide(LOCAL, None, now, None, false).unwrap_err();
        assert!(err.contains("--allow-unconfirmed-start"), "{err}");
        assert!(err.contains("--trusted-block"), "{err}");
        assert_eq!(decide(LOCAL, None, now, None, true), Ok(Anchor::Unanchored));
    }

    #[test]
    fn a_time_old_tip_with_a_trusted_block_and_no_claim_takes_the_hash_linked_path() {
        // Why: the operator opted in with a block. The hash-linked stale path
        // applies, though no head confirms the staleness.
        let now = old_now();
        assert_eq!(
            decide(LOCAL, None, now, Some(RECENT), false),
            Ok(Anchor::Stale(RECENT))
        );
        assert_eq!(
            decide(LOCAL, None, now, Some(RECENT), true),
            Ok(Anchor::Stale(RECENT))
        );
    }

    #[test]
    fn a_head_within_the_window_does_not_confirm_a_time_old_tip() {
        // Head 700 blocks past the tip, the window is 700: not beyond it.
        let now = old_now();
        let within = head(10, LOCAL.height + 700);
        assert_eq!(stale(LOCAL, within, now), None);
        assert!(decide(LOCAL, within, now, None, false).is_err());
        assert_eq!(unconfirmed_age(LOCAL, within, now), Some(WINDOW_SECS + 1));
    }

    #[test]
    fn a_time_old_tip_with_a_head_beyond_the_window_is_refused_even_with_the_flag() {
        // Why: a confirmed stale node is refused whatever the operator's flag says.
        let now = old_now();
        let ahead = head(10, LOCAL.height + 701);
        assert!(matches!(stale(LOCAL, ahead, now), Some(Stale::Time { .. })));
        let err = decide(LOCAL, ahead, now, None, false).unwrap_err();
        assert!(err.contains("--trusted-block HEIGHT:HEX"), "{err}");
        assert!(err.contains("RECENT"), "{err}");
        assert!(decide(LOCAL, ahead, now, None, true).is_err());
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
    fn a_tip_in_the_future_is_a_warning_not_a_refusal() {
        let now = 1_000_000;
        let beyond = Local {
            tip_timestamp: now + CLOCK_SKEW_SECS + 1,
            ..LOCAL
        };
        assert_eq!(future_ahead(beyond, now), Some(CLOCK_SKEW_SECS + 1));
        assert_eq!(stale(beyond, None, now), None);
        assert_eq!(
            decide(beyond, None, now, None, false),
            Ok(Anchor::Unanchored)
        );
        let within = Local {
            tip_timestamp: now + CLOCK_SKEW_SECS,
            ..LOCAL
        };
        assert_eq!(future_ahead(within, now), None);
    }

    #[test]
    fn a_stale_node_with_a_recent_trusted_block_is_anchored_and_a_fresh_one_is_checked() {
        assert_eq!(
            decide(
                LOCAL,
                head(18, 1_000),
                LOCAL.tip_timestamp,
                Some(RECENT),
                false
            ),
            Ok(Anchor::Stale(RECENT))
        );
        assert_eq!(
            decide(LOCAL, None, LOCAL.tip_timestamp, Some(TRUSTED), false),
            Ok(Anchor::Checked(TRUSTED))
        );
        assert_eq!(
            decide(LOCAL, None, LOCAL.tip_timestamp, None, false),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn a_trusted_block_older_than_the_window_behind_the_head_is_refused() {
        // Head epoch 18: height 99 is epoch 0 (too old), height 1_200 is epoch 12 (recent).
        let claim = head(18, 1_000);
        assert!(decide(LOCAL, claim, LOCAL.tip_timestamp, Some(TRUSTED), false).is_err());
        assert_eq!(
            decide(LOCAL, claim, LOCAL.tip_timestamp, Some(RECENT), false),
            Ok(Anchor::Stale(RECENT))
        );
    }

    #[test]
    fn recency_is_tightened_by_one_epoch() {
        // Why: a block exactly at the window's edge puts a follower straight back
        // into stale mid-catch-up. Head epoch 18, window 7: epoch 11 is exactly
        // one window back (refused); epoch 12 is inside it (accepted).
        let edge = TrustedBlock {
            height: 1_100,
            id: [0; 32],
        };
        let inside = TrustedBlock {
            height: 1_200,
            id: [0; 32],
        };
        assert!(!is_recent(edge, 18, 7, 100));
        assert!(is_recent(inside, 18, 7, 100));
        assert!(
            decide(
                LOCAL,
                head(18, 1_000),
                LOCAL.tip_timestamp,
                Some(edge),
                false
            )
            .is_err()
        );
        assert!(
            decide(
                LOCAL,
                head(18, 1_000),
                LOCAL.tip_timestamp,
                Some(inside),
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn recency_applies_to_a_time_stale_node_too() {
        // Once a time-old tip is confirmed by a head, the trusted block must be
        // recent against that head, whatever the stale reason.
        // Head epoch 10 (no epoch trigger), 701 blocks ahead: the time trigger.
        let now = old_now();
        let claim = head(10, LOCAL.height + 701);
        assert!(matches!(stale(LOCAL, claim, now), Some(Stale::Time { .. })));
        assert!(decide(LOCAL, claim, now, Some(TRUSTED), false).is_err());
        assert_eq!(
            decide(LOCAL, claim, now, Some(RECENT), false),
            Ok(Anchor::Stale(RECENT))
        );
    }

    #[test]
    fn recency_is_checked_against_the_maximum_claim() {
        // One source says epoch 11, another says epoch 18. The maximum (18) is
        // what the trusted block must be recent for.
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
        // The lower claim alone does not make the node stale.
        assert_eq!(
            decide(
                LOCAL,
                Some(lower),
                LOCAL.tip_timestamp,
                Some(at_epoch_10),
                false
            ),
            Ok(Anchor::Checked(at_epoch_10))
        );
        // The maximum does: epoch 10 is 8 behind epoch 18.
        assert!(
            decide(
                LOCAL,
                Some(joined),
                LOCAL.tip_timestamp,
                Some(at_epoch_10),
                false
            )
            .is_err()
        );
        assert_eq!(
            decide(
                LOCAL,
                Some(joined),
                LOCAL.tip_timestamp,
                Some(RECENT),
                false
            ),
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
        // Why: a halt leaves no newer head, so the follower keeps running (the
        // residual, documented). Only a confirmed head beyond the window, with
        // nothing pending, stops it.
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
        // A time-old follower with a trusted block still pending takes the
        // hash-linked path, though no head confirms it.
        assert_eq!(
            round_decision(Some(LOCAL), Some(RECENT), None, now),
            Ok(Anchor::Stale(RECENT))
        );
        assert_eq!(
            round_decision(None, None, confirmed, now),
            Ok(Anchor::Unanchored)
        );
    }

    #[test]
    fn the_window_saturates_rather_than_wrapping() {
        assert_eq!(window_secs(u64::MAX, 2, 15), u64::MAX);
        assert_eq!(window_secs(7, 100, 15), WINDOW_SECS);
    }

    #[test]
    fn faster_observed_blocks_shorten_the_window() {
        // 100 blocks in 500 s is 5 s per block: the window is 7 x 100 x 5 s.
        let interval = block_interval_secs(1_000, Some(500), 100);
        assert_eq!(interval, 5);
        assert_eq!(window_secs(7, 100, interval), 3_500);
        assert!(window_secs(7, 100, interval) < WINDOW_SECS);
    }

    #[test]
    fn slower_observed_blocks_are_capped_at_the_target() {
        // 100 blocks in 2,000 s is 20 s per block: the cap keeps the target, so
        // the window is never longer than the target gives.
        assert_eq!(
            block_interval_secs(3_000, Some(1_000), 100),
            TARGET_BLOCK_TIME
        );
    }

    #[test]
    fn no_history_uses_the_target() {
        // Why: a young chain has no block `epoch_blocks` back, so no measurement.
        assert_eq!(block_interval_secs(1_000, None, 100), TARGET_BLOCK_TIME);
    }

    #[test]
    fn a_non_positive_delta_uses_the_target() {
        // Why: equal or reversed timestamps say nothing about the rate.
        assert_eq!(
            block_interval_secs(1_000, Some(1_000), 100),
            TARGET_BLOCK_TIME
        );
        assert_eq!(
            block_interval_secs(1_000, Some(2_000), 100),
            TARGET_BLOCK_TIME
        );
    }

    #[test]
    fn the_interval_is_at_least_one_second() {
        // A delta under epoch_blocks seconds rounds to zero per block: clamp to 1.
        assert_eq!(block_interval_secs(1_000, Some(950), 100), 1);
    }

    #[test]
    fn a_time_old_tip_with_a_faster_chain_is_stale_sooner() {
        // Why: with 5 s blocks the window is 3,500 s. A tip 3,501 s old is then
        // time-old, where the 15 s target would still call it fresh.
        let fast = Local {
            block_interval: 5,
            ..LOCAL
        };
        let now = fast.tip_timestamp + 3_501;
        assert!(time_old(fast, now));
        assert!(!time_old(LOCAL, now));
    }

    #[test]
    fn a_block_at_or_below_the_own_tip_does_not_lift_the_unconfirmed_refusal() {
        // Why: --trusted-block set to this node's own old block would skip the
        // refusal with no catch-up. The node is at 1,000; the block is at 500.
        let own = TrustedBlock {
            height: 500,
            id: [3; 32],
        };
        let err = decide(LOCAL, None, old_now(), Some(own), false).unwrap_err();
        assert!(err.contains("--allow-unconfirmed-start"), "{err}");
        // A block above the tip still gives the hash-linked path.
        assert_eq!(
            decide(LOCAL, None, old_now(), Some(RECENT), false),
            Ok(Anchor::Stale(RECENT))
        );
    }
}
