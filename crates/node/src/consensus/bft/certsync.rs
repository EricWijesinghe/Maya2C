//! Catch-up by certificate replay (ADR-052).
//!
//! A node that was down fetches its epoch's certificates from a peer, a page
//! at a time, verifies each page before asking for the next, and writes what
//! is new into its own log. Then the ordinary restart replays that log through
//! the engine, which re-derives the blocks it is missing and checks their
//! state roots.
//!
//! The peer is trusted for nothing:
//! - a certificate counts only with a quorum of valid signatures from the
//!   epoch's committee (`verify_certificate`);
//! - a peer's `done` does not end an epoch. Completeness is anchored to the
//!   epoch's final checkpoint, which a quorum signed (ADR-038): after the replay
//!   the node's own history must contain that block at that height, so a
//!   truncated or gapped log is refused, never accepted as a shorter history.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use maya_dag_bft::{Authenticator, Certificate, Committee};

use super::attest::Checkpoint;
use super::auth::MlDsaAuthenticator;
use super::builder::unseal;
use super::store::{CertPage, SafetyStore, for_each_certificate};
use super::wire::{decode_certificate, encode_certificate};
use super::{BftDriver, BftSetup};
use crate::consensus::chain::Chain;
use crate::core::ChainTag;
use crate::crypto::validator_keys::VerifyingKey;
use crate::error::{NodeError, Result};

/// Most certificates in one page.
pub const PAGE_CERTS: usize = 256;
/// Most JSON bytes in one page, hex and framing included. Half the 10 MiB
/// default response limit of jsonrpsee, so a full page never reaches it.
pub const PAGE_ENCODED_BYTES: usize = 4 * 1024 * 1024;
/// Hard ceilings on one epoch's fetch, against a peer that never ends a
/// history. A mainnet epoch of 3,600 blocks holds about 30,000 certificates at
/// n = 4 and 50,000 at n = 7 (ADR-052 measurements): the records cap is 80 times
/// that, the bytes cap about 4 times the n = 7 log.
pub const MAX_EPOCH_PAGES: u64 = 1 << 16;
/// See [`MAX_EPOCH_PAGES`].
pub const MAX_EPOCH_RECORDS: u64 = 1 << 22;
/// See [`MAX_EPOCH_PAGES`].
pub const MAX_EPOCH_BYTES: u64 = 4 << 30;

/// How big one page may be: a record count and a JSON byte budget. The
/// production values are [`PAGE`]; tests pass tiny ones so paging runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageLimits {
    /// Most records in a page.
    pub records: usize,
    /// Most JSON bytes in a page, as [`encoded_len`] counts them.
    pub encoded_bytes: usize,
}

/// The production page limits.
pub const PAGE: PageLimits = PageLimits {
    records: PAGE_CERTS,
    encoded_bytes: PAGE_ENCODED_BYTES,
};

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

    /// The newest checkpoint the peer holds for `epoch`: for a finished epoch,
    /// its final one.
    ///
    /// # Errors
    ///
    /// Whatever the transport reports.
    fn checkpoint_of(&self, epoch: u64) -> Result<Option<Checkpoint>>;
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
    tag: ChainTag,
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
    let auth = MlDsaAuthenticator::observer(Arc::clone(keys), tag);
    let digest = c.digest();
    for (voter, signature) in c.votes.iter().zip(&c.signatures) {
        if !auth.verify(*voter, &digest, signature) {
            return Err(refuse("bad signature"));
        }
    }
    Ok(())
}

/// What one epoch's fetch did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Fetched {
    /// Certificates newly written to the local log.
    pub written: usize,
    /// Undecodable records skipped in the local log while reading it.
    pub skipped: u64,
    /// Valid certificates for a logged slot with different bytes: equivocation
    /// evidence. Counted, not logged, not used.
    pub equivocations: usize,
}

/// The end of one page: the offset after its last record, by the same count
/// the server uses.
fn page_end(offset: u64, records: &[Vec<u8>]) -> u64 {
    records.iter().fold(offset, |at, r| {
        at + 8 + u64::try_from(r.len()).unwrap_or(u64::MAX)
    })
}

/// Fetches the certificates of `epoch` that `dir` lacks, verifying each page
/// before the next is requested. A page must end where its records end, and
/// any page that is not the last must carry records: a peer cannot stall the
/// fetch, and cannot grow it past the ceilings. Already-logged certificates are
/// skipped before their signatures are checked.
///
/// `done` is taken as the peer's word here only in the sense that the fetch
/// stops; completeness is checked after the replay, in [`sync`].
///
/// # Errors
///
/// [`NodeError::Decode`] for a refused certificate, a page that does not add
/// up, a peer that makes no progress, or a ceiling exceeded; and
/// [`NodeError::Storage`] for local I/O.
pub fn fetch_epoch(
    source: &impl CertSource,
    epoch: u64,
    keys: &Arc<[VerifyingKey]>,
    weights: Option<&[u64]>,
    dir: &Path,
    limits: PageLimits,
    tag: ChainTag,
) -> Result<Fetched> {
    // Each logged slot with the digest of its encoded bytes: a peer's copy of
    // a logged slot is skipped only if it is byte-identical, and otherwise
    // refused, so a forged copy is not silently passed over.
    let mut have = BTreeMap::new();
    let skipped = for_each_certificate(dir, epoch, |c| {
        have.insert(
            (c.vertex.round, c.vertex.author),
            blake3::hash(&encode_certificate(&c)),
        );
        Ok(())
    })?;
    let (mut store, _) = SafetyStore::open(dir, epoch)?;
    let mut fetched = Fetched {
        written: 0,
        skipped,
        equivocations: 0,
    };
    let (mut offset, mut pages, mut records, mut bytes) = (0_u64, 0_u64, 0_u64, 0_u64);
    loop {
        pages += 1;
        if pages > MAX_EPOCH_PAGES {
            return Err(refuse("the epoch takes more pages than allowed"));
        }
        let page = source.page(epoch, offset, limits.records)?;
        if page.records.len() > limits.records {
            return Err(refuse("page larger than asked"));
        }
        let end = page_end(offset, &page.records);
        if page.next != end {
            return Err(refuse("page end does not match its records"));
        }
        if !page.done && page.records.is_empty() {
            return Err(refuse("peer made no progress"));
        }
        records += u64::try_from(page.records.len()).unwrap_or(u64::MAX);
        bytes += end - offset;
        if records > MAX_EPOCH_RECORDS || bytes > MAX_EPOCH_BYTES {
            return Err(refuse("the epoch is larger than allowed"));
        }
        for record in &page.records {
            let c = decode_certificate(record).map_err(|_| refuse("undecodable"))?;
            let slot = (c.vertex.round, c.vertex.author);
            let digest = blake3::hash(&encode_certificate(&c));
            match have.get(&slot) {
                Some(logged) if *logged == digest => continue,
                Some(_) => {
                    // A different copy of a logged slot must still be valid to
                    // count. Two valid certificates for one slot are two quorums
                    // signing two vertices: evidence of equivocation. It is kept
                    // out of the log and the first copy stays. Forged copies are
                    // refused, so a peer cannot stall catch-up with one.
                    verify_certificate(&c, epoch, keys, weights, tag)?;
                    fetched.equivocations += 1;
                    continue;
                }
                None => {}
            }
            verify_certificate(&c, epoch, keys, weights, tag)?;
            store.record_certificate(&c)?;
            have.insert(slot, digest);
            fetched.written += 1;
        }
        offset = page.next;
        if page.done {
            return Ok(fetched);
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
    let (keys, weights) =
        BftDriver::staked_committee(chain)?.unwrap_or_else(|| (Arc::clone(&setup.committee), None));
    Ok((epoch, (keys, weights)))
}

/// Completeness of a replayed epoch (ADR-038, ADR-052). The epoch's checkpoint
/// is a block a quorum signed; the replayed history must contain that block at
/// that height. A finished epoch with no checkpoint on offer is refused, since
/// nothing then says where it ends. The head epoch may have none yet.
fn attested_end(
    chain: &Chain,
    source: &impl CertSource,
    epoch: u64,
    head: u64,
    keys: &Arc<[VerifyingKey]>,
    weights: Option<&[u64]>,
) -> Result<()> {
    let Some(checkpoint) = source.checkpoint_of(epoch)? else {
        return if epoch < head {
            Err(refuse("no attested end for a finished epoch"))
        } else {
            Ok(())
        };
    };
    if checkpoint.epoch != epoch {
        return Err(refuse("checkpoint of another epoch"));
    }
    let tag = ChainTag::from_genesis(chain.genesis());
    checkpoint.verify(&tag, keys, weights)?;
    if checkpoint.height > chain.height() {
        // Newer than the replayed history: nothing to check it against yet. A
        // finished epoch must reach its own end, so a short replay is refused.
        return if epoch < head {
            Err(refuse(
                "the replayed history stops short of the attested end",
            ))
        } else {
            Ok(())
        };
    }
    let ids = chain.active_chain()?;
    if ids.get(usize::try_from(checkpoint.height).unwrap_or(usize::MAX)) != Some(&checkpoint.block)
    {
        return Err(refuse(
            "the replayed history does not reproduce the attested checkpoint",
        ));
    }
    if epoch < head && last_height_of(chain, &ids, epoch) != Some(checkpoint.height) {
        // An older checkpoint of a finished epoch proves nothing about its end,
        // and a peer can serve one (the book keeps the newest it has seen).
        return Err(refuse(
            "the attested checkpoint is not the last block of a finished epoch",
        ));
    }
    Ok(())
}

/// The height of the last block whose seal names `epoch`: the epoch's final
/// block, which is where a finished epoch's checkpoint must sit.
fn last_height_of(chain: &Chain, ids: &[[u8; 32]], epoch: u64) -> Option<u64> {
    (0..ids.len()).rev().find_map(|h| {
        let block = chain.get(&ids[h])?;
        (unseal(block.header.nonce).0 == epoch).then_some(u64::try_from(h).ok()?)
    })
}

/// What [`sync`] leaves behind: the epoch boundaries crossed, and the driver
/// its last replay opened, with the frames that open produced.
pub struct Synced {
    /// Epoch boundaries crossed.
    pub crossed: u64,
    /// Undecodable records skipped while reading the local logs.
    pub skipped: u64,
    /// Valid equivocating copies of logged slots seen while fetching.
    pub equivocations: usize,
    /// The engine, replayed to the end of the fetched logs.
    pub driver: BftDriver,
    /// What opening the engine produced: frames to send, notices.
    pub step: super::Step,
}

/// Catches `chain` up to the peer's epoch, one epoch at a time: fetch the
/// epoch's certificates, replay them (which builds blocks through any
/// boundary and moves the state on), check the replayed history against the
/// epoch's attested end, then the next epoch. The last replay's driver is
/// returned, so a caller does not replay the same logs twice.
///
/// # Errors
///
/// As [`fetch_epoch`], and [`NodeError::Storage`] or [`NodeError::Decode`]
/// from the replay or the attested-end check. On error the chain may hold the
/// blocks replayed so far, all of them derived from verified certificates; the
/// caller falls back and does not treat the node as caught up.
pub fn sync(
    chain: &mut Chain,
    source: &impl CertSource,
    setup: &BftSetup,
    dir: &Path,
    now_ms: u64,
    limits: PageLimits,
) -> Result<Synced> {
    let (mut crossed, mut skipped, mut equivocations) = (0, 0, 0);
    loop {
        let (epoch, (keys, weights)) = current(chain, setup)?;
        let head = source.current_epoch()?;
        let behind = epoch <= head;
        if behind {
            let fetched = fetch_epoch(
                source,
                epoch,
                &keys,
                weights.as_deref(),
                dir,
                limits,
                ChainTag::from_genesis(chain.genesis()),
            )?;
            skipped += fetched.skipped;
            equivocations += fetched.equivocations;
        }
        let (driver, step) = BftDriver::open(setup, dir, chain, now_ms)?;
        if behind {
            attested_end(chain, source, epoch, head, &keys, weights.as_deref())?;
        }
        let (after, _) = current(chain, setup)?;
        // The peer has finished this epoch, so its replay must reach the
        // boundary. Stopping short is a truncated log, however it was served.
        if behind && after == epoch && epoch < head {
            return Err(refuse(
                "the peer has finished this epoch, but the replay stops before its boundary",
            ));
        }
        if !behind || after == epoch || after > head {
            return Ok(Synced {
                crossed,
                skipped,
                equivocations,
                driver,
                step,
            });
        }
        crossed += 1;
    }
}
