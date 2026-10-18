//! Catch-up by certificate replay (ADR-052).
//!
//! A node that was down fetches its epoch's certificates from a peer, a page
//! at a time, verifies each page before asking for the next, and writes what
//! is new into its own log. Then the ordinary restart replays that log through
//! the engine, which re-derives the blocks it is missing and checks their
//! state roots. The peer is trusted for nothing: a certificate counts only
//! with a quorum of valid signatures from the epoch's committee.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use maya_dag_bft::{Authenticator, Certificate, Committee};

use super::auth::MlDsaAuthenticator;
use super::store::{CertPage, SafetyStore, for_each_certificate};
use super::wire::decode_certificate;
use super::{BftDriver, BftSetup};
use crate::consensus::chain::Chain;
use crate::crypto::keys::VerifyingKey;
use crate::error::{NodeError, Result};

/// Most certificates in one page. A client refuses a larger one, and a server
/// never serves one: it bounds what either side holds at once.
pub const PAGE_CERTS: usize = 256;
/// Most bytes a server puts in one page, beyond which the page ends early.
pub const PAGE_BYTES: usize = 8 * 1024 * 1024;

/// Where a catching-up node gets certificates: a peer's log, over RPC or in
/// process (tests).
pub trait CertSource {
    /// The epoch the peer is running.
    ///
    /// # Errors
    ///
    /// Whatever the transport reports.
    fn current_epoch(&self) -> Result<u64>;

    /// Up to `max_records` certificate records of `epoch` from byte `offset`.
    ///
    /// # Errors
    ///
    /// Whatever the transport reports, including a peer that no longer keeps
    /// the epoch's log.
    fn page(&self, epoch: u64, offset: u64, max_records: usize) -> Result<CertPage>;
}

fn refuse(what: &str) -> NodeError {
    NodeError::Decode(format!("certificate refused: {what}"))
}

/// Checks one certificate against the committee of `epoch`: its epoch, a
/// quorum of its votes by the engine's own rule, and one valid signature per
/// vote over its digest.
///
/// # Errors
///
/// [`NodeError::Decode`] for a certificate of another epoch, insufficient
/// evidence, a vote count that does not match its signatures, or any bad
/// signature.
pub fn verify_certificate(
    c: &Certificate,
    epoch: u64,
    keys: &Arc<[VerifyingKey]>,
    weights: Option<&[u64]>,
) -> Result<()> {
    if c.vertex.epoch != epoch {
        return Err(refuse("another epoch"));
    }
    if c.votes.len() != c.signatures.len() {
        return Err(refuse("votes and signatures differ in number"));
    }
    let committee = match weights {
        Some(w) => Committee::weighted(w.to_vec()),
        None => u16::try_from(keys.len()).ok().map(Committee::new),
    }
    .ok_or_else(|| refuse("committee size"))?;
    if !c.is_well_formed(&committee) {
        return Err(refuse("insufficient evidence"));
    }
    let auth = MlDsaAuthenticator::observer(Arc::clone(keys));
    let digest = c.digest();
    for (voter, signature) in c.votes.iter().zip(&c.signatures) {
        if !auth.verify(*voter, &digest, signature) {
            return Err(refuse("bad signature"));
        }
    }
    Ok(())
}

/// Fetches the certificates of `epoch` that `dir` lacks, verifying each page
/// before the next is requested. Returns how many were written.
///
/// # Errors
///
/// [`NodeError::Decode`] for a refused certificate or a peer that makes no
/// progress or sends an oversized page; [`NodeError::Storage`] for local I/O.
pub fn fetch_epoch(
    source: &impl CertSource,
    epoch: u64,
    keys: &Arc<[VerifyingKey]>,
    weights: Option<&[u64]>,
    dir: &Path,
) -> Result<usize> {
    let mut have = BTreeSet::new();
    for_each_certificate(dir, epoch, |c| {
        have.insert((c.vertex.round, c.vertex.author));
        Ok(())
    })?;
    let (mut store, _) = SafetyStore::open(dir, epoch)?;
    let mut offset = 0_u64;
    let mut written = 0;
    loop {
        let page = source.page(epoch, offset, PAGE_CERTS)?;
        if page.records.len() > PAGE_CERTS {
            return Err(refuse("page larger than asked"));
        }
        if page.records.is_empty() && !page.done || page.next < offset {
            return Err(refuse("peer made no progress"));
        }
        for record in &page.records {
            let c = decode_certificate(record).map_err(|_| refuse("undecodable"))?;
            verify_certificate(&c, epoch, keys, weights)?;
            if have.insert((c.vertex.round, c.vertex.author)) {
                store.record_certificate(&c)?;
                written += 1;
            }
        }
        offset = page.next;
        if page.done {
            return Ok(written);
        }
    }
}

/// An epoch's committee keys and, on a stake-weighted chain, its weights.
type EpochKeys = (Arc<[VerifyingKey]>, Option<Arc<[u64]>>);

/// The epoch the chain's state is in, and its committee.
fn current(chain: &Chain, setup: &BftSetup) -> Result<(u64, EpochKeys)> {
    let epoch = chain
        .state()
        .committed_staking()?
        .map_or(setup.epoch, |record| record.staking.epoch);
    let (keys, weights) = BftDriver::staked_committee(chain)?
        .unwrap_or_else(|| (Arc::clone(&setup.committee), None));
    Ok((epoch, (keys, weights)))
}

/// What [`sync`] leaves behind: the epoch boundaries crossed, and the driver
/// its last replay opened, with the frames that open produced.
pub struct Synced {
    /// Epoch boundaries crossed.
    pub crossed: u64,
    /// The engine, replayed to the end of the fetched logs.
    pub driver: BftDriver,
    /// What opening the engine produced: frames to send, notices.
    pub step: super::Step,
}

/// Catches `chain` up to the peer's epoch, one epoch at a time: fetch the
/// epoch's certificates, replay them (which builds blocks through any
/// boundary and moves the state on), then the next epoch. The last replay's
/// driver is returned, so a caller does not replay the same logs twice.
///
/// # Errors
///
/// As [`fetch_epoch`], and [`NodeError::Storage`] or [`NodeError::Decode`]
/// from the replay.
pub fn sync(
    chain: &mut Chain,
    source: &impl CertSource,
    setup: &BftSetup,
    dir: &Path,
    now_ms: u64,
) -> Result<Synced> {
    let mut crossed = 0;
    loop {
        let (epoch, (keys, weights)) = current(chain, setup)?;
        let head = source.current_epoch()?;
        let behind = epoch <= head;
        if behind {
            fetch_epoch(source, epoch, &keys, weights.as_deref(), dir)?;
        }
        let (driver, step) = BftDriver::open(setup, dir, chain, now_ms)?;
        let (after, _) = current(chain, setup)?;
        if !behind || after == epoch || after > head {
            return Ok(Synced {
                crossed,
                driver,
                step,
            });
        }
        crossed += 1;
    }
}
