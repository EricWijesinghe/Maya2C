//! Refuses to start a validator that has lost its BFT safety log (audit L-2).
//!
//! The engine fsyncs each of its own proposals and votes to
//! `<data-dir>/bft/epoch-N/safety.log` before sending it, and replays the log
//! on start so it never signs a round twice. A validator restarted onto a
//! fresh or restored data directory has no log: it could sign again a round it
//! already signed, which is equivocation, and equivocation is slashed.
//!
//! A remote signer (`--remote-signer`) is not checked: `maya2c-signer` keeps
//! its own fsynced slashing-protection database on the signer host (one record
//! per `(round, author)` slot, and a proposal watermark; `crates/signer`
//! `protection.rs`), so a lost node data directory does not lose it.
//!
//! shortcut: a chain at height 0 is never refused, because a genesis launch
//! looks the same as a wiped validator before it catches up. A wiped validator
//! that restarts at genesis and catches up is not caught here; upgrade by
//! asking the committee's certificates whether this key signed in the epoch.

use std::error::Error;
use std::path::Path;
use std::sync::Mutex;

use custom_l1_node::consensus::Chain;
use custom_l1_node::consensus::bft::remote::ValidatorKey;
use custom_l1_node::consensus::bft::store::SAFETY_LOG;
use custom_l1_node::consensus::bft::{BftDriver, BftSetup};

use super::bft::BFT_DIR;
use super::{lock_chain, rejoin};

/// Whether `bft_dir` holds the safety log of `epoch`, the epoch the driver
/// will run.
///
/// Another epoch's log says nothing about this one's rounds: the engine signs
/// each epoch's rounds afresh and replays only that epoch's log, and the
/// previous epoch's directory is kept after a new one starts, so it outlives
/// the loss of the current one. A bare directory does not count either: a
/// deleted log leaves an empty or certs-only directory behind.
fn holds_safety_log(bft_dir: &Path, epoch: u64) -> std::io::Result<bool> {
    let path = bft_dir.join(format!("epoch-{epoch}")).join(SAFETY_LOG);
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// What the decision needs, read from the node.
// A decision table: each bool is one independent fact, not a state.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug)]
struct Facts {
    /// A local key (not a remote signer) in the current epoch's committee.
    local_member: bool,
    holds_log: bool,
    height: u64,
    /// The tip is within the unbonding window, or the chain has no staking.
    recent: bool,
    never_signed: bool,
}

/// The refusal, or `Ok` when the node may start.
fn decide(f: Facts, bft_dir: &Path) -> Result<(), String> {
    if !f.local_member || f.holds_log || f.height == 0 || !f.recent || f.never_signed {
        return Ok(());
    }
    Err(format!(
        "refusing to start this validator: its BFT safety log ({}) is missing, and its chain \
         (height {}) is recent. The log is how a restarted validator knows which rounds it \
         already signed. Without it this node can sign a round a second time: that is \
         equivocation, the network records it as evidence, and the stake is slashed. \
         Restore the old <data-dir>/bft directory if you have it (never delete bft/). If this \
         key has signed nothing in the current epoch (a new validator, one down since an \
         earlier epoch, or one stopped at an epoch boundary before it signed in the new \
         epoch), start again with --i-know-this-validator-never-signed",
        bft_dir.display(),
        f.height
    ))
}

/// Refuses a committee validator whose safety log is missing on a recent chain,
/// unless the operator passed `--i-know-this-validator-never-signed`.
///
/// # Errors
///
/// The refusal above, or a failure reading the chain or `bft/`.
pub(super) fn check(
    setup: &BftSetup,
    data_dir: &Path,
    chain: &Mutex<Chain>,
    never_signed: bool,
) -> Result<(), Box<dyn Error>> {
    let Some(ValidatorKey::Local(key)) = &setup.signer else {
        return Ok(());
    };
    let bft_dir = data_dir.join(BFT_DIR);
    let (local_member, height, recent, epoch) = {
        let c = lock_chain(chain);
        // The epoch the engine will run, chosen as `BftDriver::open` does.
        let epoch = c
            .state()
            .committed_staking()?
            .map_or(setup.epoch, |record| record.staking.epoch);
        let committee = BftDriver::staked_committee(&c)?
            .map_or_else(|| std::sync::Arc::clone(&setup.committee), |(keys, _)| keys);
        let me = key.verifying_key();
        let recent = match rejoin::local_state(&c)? {
            Some(local) => !rejoin::time_old(local, rejoin::clock_now()),
            None => true,
        };
        (committee.contains(&me), c.height(), recent, epoch)
    };
    let facts = Facts {
        local_member,
        holds_log: holds_safety_log(&bft_dir, epoch)?,
        height,
        recent,
        never_signed,
    };
    decide(facts, &bft_dir).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const WIPED: Facts = Facts {
        local_member: true,
        holds_log: false,
        height: 500,
        recent: true,
        never_signed: false,
    };

    #[test]
    fn a_recent_validator_without_its_safety_log_is_refused() {
        let err = decide(WIPED, Path::new("bft")).unwrap_err();
        assert!(err.contains("equivocation"), "{err}");
        assert!(
            err.contains("--i-know-this-validator-never-signed"),
            "{err}"
        );
    }

    #[test]
    fn the_refusal_has_exactly_the_stated_exceptions() {
        let ok = |f: Facts| decide(f, Path::new("bft")).is_ok();
        assert!(ok(Facts {
            never_signed: true,
            ..WIPED
        }));
        assert!(ok(Facts {
            holds_log: true,
            ..WIPED
        }));
        assert!(ok(Facts {
            local_member: false,
            ..WIPED
        }));
        assert!(ok(Facts { height: 0, ..WIPED }));
        assert!(ok(Facts {
            recent: false,
            ..WIPED
        }));
    }

    #[test]
    fn only_the_current_epochs_safety_log_counts() {
        let root = tempfile::TempDir::new().unwrap();
        let bft = root.path().join(BFT_DIR);
        assert!(!holds_safety_log(&bft, 3).unwrap(), "missing");
        std::fs::create_dir_all(&bft).unwrap();
        std::fs::write(bft.join("notes.txt"), b"x").unwrap();
        assert!(!holds_safety_log(&bft, 3).unwrap(), "no epoch directory");
        // The previous epoch's log is kept after the current one starts, so it
        // survives the current one's loss and must not count for it.
        std::fs::create_dir_all(bft.join("epoch-2")).unwrap();
        std::fs::write(bft.join("epoch-2").join(SAFETY_LOG), b"x").unwrap();
        assert!(!holds_safety_log(&bft, 3).unwrap(), "only an older epoch");
        std::fs::create_dir_all(bft.join("epoch-3")).unwrap();
        assert!(!holds_safety_log(&bft, 3).unwrap(), "empty epoch directory");
        std::fs::write(bft.join("epoch-3").join("certs.log"), b"x").unwrap();
        assert!(
            !holds_safety_log(&bft, 3).unwrap(),
            "certs log is not a safety log"
        );
        std::fs::write(bft.join("epoch-3").join(SAFETY_LOG), b"x").unwrap();
        assert!(holds_safety_log(&bft, 3).unwrap());
        assert!(holds_safety_log(&bft, 2).unwrap(), "each epoch's own log");
    }
}
