//! A satellite-data oracle for parametric insurance (Master Prompt 6 §8):
//! NDVI from Sentinel-2 pixels, several independent reporters, a median, a
//! dispute window, and a policy that pays when the field's vegetation falls
//! below its trigger.
//!
//! NDVI is computed in integers (basis points, −10,000 to 10,000) so any
//! verifier reproduces a reporter's figure exactly. Each report commits to
//! the pixels it was computed from, and counts only once those pixels are
//! opened during the dispute window — by its reporter or anyone holding them
//! — and its NDVI reproduces from them; one that does not reproduce is
//! struck. After the window, the value is the median of the opened reports,
//! from at least [`MIN_SOURCES`] reporters. A commitment nobody opens counts
//! for nothing, so a report cannot rest on pixels that do not exist.
//!
//! **What a dispute cannot prove:** that the committed pixels really came
//! from the satellite. Sentinel-2 imagery is not signed by its operator, so
//! provenance rests on the reporters being independent — which is what the
//! median is for — not on cryptography.
//!
//! RESEARCH: nothing in the node calls this crate.

use std::collections::{BTreeMap, BTreeSet};

use maya_crypto_pq::suite::{self, MlDsa65, SignatureSuite, SuiteId};

/// Fewest distinct reporters a final value needs.
pub const MIN_SOURCES: usize = 3;
/// NDVI scale: 10,000 = 1.0.
pub const BPS: i32 = 10_000;
/// Sentinel-2 L2A `BOA_ADD_OFFSET` from processing baseline 04.00.
pub const BOA_ADD_OFFSET: i32 = -1_000;

/// Why the oracle refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OracleError {
    /// A report's signature does not verify under a registered key.
    #[error("bad signature")]
    BadSignature,
    /// An unregistered reporter.
    #[error("unknown reporter")]
    UnknownReporter,
    /// One report per reporter per round.
    #[error("already reported")]
    Duplicate,
    /// Outside the round's phase for this action.
    #[error("{0}")]
    Phase(&'static str),
    /// Dispute evidence that does not open the report's commitment.
    #[error("the pixels do not open the report's commitment")]
    NotTheCommittedData,
    /// The pixels hold no valid NDVI.
    #[error("no valid pixel")]
    NoData,
    /// Too few reports standing to finalize.
    #[error("{0} reports standing, {MIN_SOURCES} needed")]
    TooFewSources(usize),
}

/// NDVI of one pixel from L2A digital numbers, in basis points; `None` where
/// both bands are dark (no signal).
#[must_use]
pub fn ndvi_bps(red_dn: u16, nir_dn: u16) -> Option<i32> {
    let red = (i32::from(red_dn) + BOA_ADD_OFFSET).max(0);
    let nir = (i32::from(nir_dn) + BOA_ADD_OFFSET).max(0);
    let sum = red + nir;
    (sum > 0).then(|| (nir - red) * BPS / sum)
}

/// The field's NDVI: the median over valid pixels (the lower one for an
/// even count), which a few cloud or water pixels cannot drag.
///
/// # Errors
///
/// [`OracleError::NoData`] for mismatched bands or no valid pixel.
pub fn field_ndvi(red: &[u16], nir: &[u16]) -> Result<i32, OracleError> {
    if red.len() != nir.len() {
        return Err(OracleError::NoData);
    }
    let mut values: Vec<i32> = red
        .iter()
        .zip(nir)
        .filter_map(|(r, n)| ndvi_bps(*r, *n))
        .collect();
    if values.is_empty() {
        return Err(OracleError::NoData);
    }
    values.sort_unstable();
    Ok(values[(values.len() - 1) / 2])
}

/// The commitment to a window of pixels.
#[must_use]
pub fn data_commitment(red: &[u16], nir: &[u16]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("maya2c eo-oracle pixels v1");
    h.update(&u64::try_from(red.len()).unwrap_or(u64::MAX).to_le_bytes());
    h.update(&u64::try_from(nir.len()).unwrap_or(u64::MAX).to_le_bytes());
    red.iter().chain(nir).for_each(|v| {
        h.update(&v.to_le_bytes());
    });
    *h.finalize().as_bytes()
}

/// A reporter's claim for one round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Round (e.g. an epoch number).
    pub round: u64,
    /// The field, e.g. a hash of its polygon.
    pub field: [u8; 32],
    /// NDVI claimed, basis points.
    pub ndvi_bps: i32,
    /// Commitment to the pixels it came from.
    pub data: [u8; 32],
    /// Which scene, e.g. `S2A_10SGG_20250616_0_L2A` (informational).
    pub scene: String,
}

impl Report {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut out = b"maya2c eo-oracle report v1".to_vec();
        out.extend_from_slice(&self.round.to_le_bytes());
        out.extend_from_slice(&self.field);
        out.extend_from_slice(&self.ndvi_bps.to_le_bytes());
        out.extend_from_slice(&self.data);
        out.extend_from_slice(
            &u64::try_from(self.scene.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(self.scene.as_bytes());
        out
    }

    /// Signs as `key`'s holder.
    ///
    /// # Errors
    ///
    /// The signer refused.
    pub fn sign(
        &self,
        key: &<MlDsa65 as SignatureSuite>::SigningKey,
    ) -> Result<Vec<u8>, OracleError> {
        MlDsa65::sign(key, &self.signing_bytes()).map_err(|_| OracleError::BadSignature)
    }
}

/// One round for one field.
#[derive(Clone, Debug)]
pub struct Round {
    id: u64,
    field: [u8; 32],
    opens_at: u64,
    closes_at: u64,
    finalizes_at: u64,
    reporters: BTreeMap<[u8; 32], Vec<u8>>,
    reports: BTreeMap<[u8; 32], Report>,
    struck: BTreeSet<[u8; 32]>,
    /// Reports whose pixels were opened and reproduced. Only these count: a
    /// commitment nobody opens could hide anything, including pixels that
    /// never existed.
    opened: BTreeSet<[u8; 32]>,
}

impl Round {
    /// A round accepting reports in `[opens_at, closes_at)` and disputes in
    /// `[closes_at, finalizes_at)`, from the registered `reporters`
    /// (id → ML-DSA-65 public key).
    #[must_use]
    pub fn new(
        round: u64,
        field: [u8; 32],
        (opens_at, closes_at, finalizes_at): (u64, u64, u64),
        reporters: BTreeMap<[u8; 32], Vec<u8>>,
    ) -> Self {
        Self {
            id: round,
            field,
            opens_at,
            closes_at,
            finalizes_at,
            reporters,
            reports: BTreeMap::new(),
            struck: BTreeSet::new(),
            opened: BTreeSet::new(),
        }
    }

    /// Accepts a signed report during the reporting phase.
    ///
    /// # Errors
    ///
    /// Wrong phase, round or field, an unknown reporter, a bad signature, or
    /// a second report.
    pub fn submit(
        &mut self,
        reporter: [u8; 32],
        report: Report,
        signature: &[u8],
        now: u64,
    ) -> Result<(), OracleError> {
        if now < self.opens_at || now >= self.closes_at {
            return Err(OracleError::Phase(
                "reports are accepted only while the round is open",
            ));
        }
        if report.round != self.id || report.field != self.field {
            return Err(OracleError::Phase("a report for another round or field"));
        }
        let key = self
            .reporters
            .get(&reporter)
            .ok_or(OracleError::UnknownReporter)?;
        suite::verify(SuiteId::MlDsa65, key, &report.signing_bytes(), signature)
            .map_err(|_| OracleError::BadSignature)?;
        if self.reports.contains_key(&reporter) {
            return Err(OracleError::Duplicate);
        }
        self.reports.insert(reporter, report);
        Ok(())
    }

    /// Opens a report's pixels during the dispute window: the reporter
    /// publishes them, or anyone who has them does. A report whose NDVI
    /// reproduces is counted; one that does not is struck. Opening a report
    /// again changes nothing, so repeated openings cost the caller, not the
    /// round. Returns whether the report was struck.
    ///
    /// # Errors
    ///
    /// Outside the dispute window, no such report, or pixels that are not the
    /// committed ones.
    pub fn open(
        &mut self,
        reporter: [u8; 32],
        red: &[u16],
        nir: &[u16],
        now: u64,
    ) -> Result<bool, OracleError> {
        if now < self.closes_at || now >= self.finalizes_at {
            return Err(OracleError::Phase(
                "pixels are opened only in the dispute window",
            ));
        }
        let report = self
            .reports
            .get(&reporter)
            .ok_or(OracleError::UnknownReporter)?;
        if data_commitment(red, nir) != report.data {
            return Err(OracleError::NotTheCommittedData);
        }
        if self.struck.contains(&reporter) || self.opened.contains(&reporter) {
            return Ok(self.struck.contains(&reporter));
        }
        let wrong = field_ndvi(red, nir) != Ok(report.ndvi_bps);
        if wrong {
            self.struck.insert(reporter);
        } else {
            self.opened.insert(reporter);
        }
        Ok(wrong)
    }

    /// The final NDVI: the median (lower, for an even count) of the reports
    /// opened and reproduced, once the dispute window has passed. A report
    /// never opened does not count.
    ///
    /// # Errors
    ///
    /// Before the window ends, or fewer than [`MIN_SOURCES`] standing.
    pub fn finalize(&self, now: u64) -> Result<i32, OracleError> {
        if now < self.finalizes_at {
            return Err(OracleError::Phase("the dispute window is still open"));
        }
        let mut standing: Vec<i32> = self
            .reports
            .iter()
            .filter(|(r, _)| self.opened.contains(*r))
            .map(|(_, rep)| rep.ndvi_bps)
            .collect();
        if standing.len() < MIN_SOURCES {
            return Err(OracleError::TooFewSources(standing.len()));
        }
        standing.sort_unstable();
        Ok(standing[(standing.len() - 1) / 2])
    }

    /// Reporters whose reports were struck.
    #[must_use]
    pub fn struck(&self) -> &BTreeSet<[u8; 32]> {
        &self.struck
    }
}

/// A parametric policy: pays `payout` if the field's final NDVI falls below
/// `trigger_bps` — drought or crop failure — with no claim or loss adjuster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The insured field.
    pub field: [u8; 32],
    /// Pays below this NDVI.
    pub trigger_bps: i32,
    /// The payout.
    pub payout: u64,
}

impl Policy {
    /// What the policy pays for a finalized round.
    #[must_use]
    pub fn settle(&self, final_ndvi_bps: i32) -> u64 {
        if final_ndvi_bps < self.trigger_bps {
            self.payout
        } else {
            0
        }
    }
}
