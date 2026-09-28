//! A header chain: every header's consensus checks, most-chainwork fork
//! choice, and reorgs.
//!
//! # Checks per header
//!
//! 1. The parent is known.
//! 2. `bits` is exactly what the retarget rule demands at that height.
//! 3. The hash meets that target.
//! 4. The timestamp exceeds the median of the previous eleven (median time
//!    past). Near a checkpoint fewer ancestors exist and the median is over
//!    those — a documented weakening, not a silent one.
//!
//! **Not checked:** the "no more than two hours in the future" rule, because
//! it reads a wall clock and this crate must give every node the same answer
//! for the same bytes. A relayer that feeds future-dated headers still has to
//! out-work the honest chain to be believed.

use std::collections::BTreeMap;

use crate::header::{
    BlockHash, Header, compact_from_target, target_from_compact, work_from_target,
};
use crate::u256::U256;

/// Network parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Easiest permitted target, compact.
    pub pow_limit_bits: u32,
    /// Blocks per retarget period.
    pub interval: u32,
    /// Intended seconds per period.
    pub target_timespan: u32,
    /// Regtest never retargets.
    pub no_retargeting: bool,
}

impl Params {
    /// Bitcoin mainnet.
    pub const MAINNET: Self = Self {
        pow_limit_bits: 0x1d00_ffff,
        interval: 2016,
        target_timespan: 14 * 24 * 60 * 60,
        no_retargeting: false,
    };

    /// Bitcoin regtest: minimal difficulty, no retargeting. Real parameters,
    /// used by tests that need to mine headers.
    pub const REGTEST: Self = Self {
        pow_limit_bits: 0x207f_ffff,
        interval: 2016,
        target_timespan: 14 * 24 * 60 * 60,
        no_retargeting: true,
    };

    /// The next period's compact target given the previous period's first
    /// and last timestamps and the last compact target (Core's
    /// `CalculateNextWorkRequired`).
    pub fn retarget(&self, first_time: u32, last_time: u32, last_bits: u32) -> Option<u32> {
        let span = i64::from(last_time) - i64::from(first_time);
        let min = i64::from(self.target_timespan / 4);
        let max = i64::from(self.target_timespan) * 4;
        let actual = u64::try_from(span.clamp(min, max)).ok()?;
        let limit = target_from_compact(self.pow_limit_bits)?;
        let next = target_from_compact(last_bits)?
            .checked_mul_u64(actual)?
            .div_u64(u64::from(self.target_timespan));
        Some(compact_from_target(next.min(limit)))
    }
}

/// Why a header was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// Parent not in the chain.
    UnknownParent,
    /// `bits` differs from what the retarget rule requires.
    WrongDifficulty {
        /// Required.
        expected: u32,
        /// Presented.
        found: u32,
    },
    /// Hash above the target.
    InsufficientWork,
    /// Timestamp not after the median of the previous eleven.
    TimeTooOld,
    /// The ancestors a retarget needs are before the checkpoint.
    RetargetBeforeCheckpoint,
    /// Already held.
    Duplicate,
}

#[derive(Clone, Debug)]
struct Entry {
    header: Header,
    height: u32,
    chainwork: U256,
}

/// A validated header tree with a best tip.
#[derive(Clone, Debug)]
pub struct HeaderChain {
    params: Params,
    entries: BTreeMap<BlockHash, Entry>,
    tip: BlockHash,
    root_height: u32,
}

impl HeaderChain {
    /// Starts from a trusted checkpoint header at `height`. The checkpoint
    /// should sit on a retarget boundary (`height % interval == 0`) so the next
    /// retarget has its period-start header; genesis (height 0) does.
    pub fn from_checkpoint(params: Params, height: u32, header: Header) -> Self {
        let chainwork = target_from_compact(header.bits).map_or(U256::ZERO, work_from_target);
        let hash = header.hash();
        let mut entries = BTreeMap::new();
        entries.insert(
            hash,
            Entry {
                header,
                height,
                chainwork,
            },
        );
        Self {
            params,
            entries,
            tip: hash,
            root_height: height,
        }
    }

    /// Best tip hash.
    pub fn tip(&self) -> BlockHash {
        self.tip
    }

    /// A header this chain holds, on the best chain or a side branch; a
    /// caller checks [`HeaderChain::confirmations`] before trusting it.
    pub fn header(&self, hash: &BlockHash) -> Option<&Header> {
        self.entries.get(hash).map(|e| &e.header)
    }

    /// Best tip height.
    pub fn height(&self) -> u32 {
        self.entries.get(&self.tip).map_or(0, |e| e.height)
    }

    /// Cumulative work of the best tip since the checkpoint.
    pub fn chainwork(&self) -> U256 {
        self.entries
            .get(&self.tip)
            .map_or(U256::ZERO, |e| e.chainwork)
    }

    /// Confirmations of `hash` on the best chain (1 = it is the tip), or 0 if
    /// it is not on the best chain.
    pub fn confirmations(&self, hash: &BlockHash) -> u32 {
        let Some(target) = self.entries.get(hash) else {
            return 0;
        };
        let mut cursor = self.tip;
        while let Some(e) = self.entries.get(&cursor) {
            if e.height == target.height {
                return if cursor == *hash {
                    self.height() - target.height + 1
                } else {
                    0
                };
            }
            if e.height < target.height {
                return 0;
            }
            cursor = e.header.prev;
        }
        0
    }

    fn ancestor(&self, from: &BlockHash, height: u32) -> Option<&Entry> {
        let mut cursor = *from;
        loop {
            let e = self.entries.get(&cursor)?;
            if e.height == height {
                return Some(e);
            }
            if e.height < height {
                return None;
            }
            cursor = e.header.prev;
        }
    }

    fn expected_bits(&self, parent: &Entry) -> Result<u32, HeaderError> {
        let height = parent.height + 1;
        if self.params.no_retargeting || !height.is_multiple_of(self.params.interval) {
            return Ok(parent.header.bits);
        }
        let first_height = height - self.params.interval;
        if first_height < self.root_height {
            return Err(HeaderError::RetargetBeforeCheckpoint);
        }
        let first = self
            .ancestor(&parent.header.hash(), first_height)
            .ok_or(HeaderError::RetargetBeforeCheckpoint)?;
        self.params
            .retarget(first.header.time, parent.header.time, parent.header.bits)
            .ok_or(HeaderError::WrongDifficulty {
                expected: 0,
                found: parent.header.bits,
            })
    }

    fn median_time_past(&self, parent: &Entry) -> u32 {
        let mut times = Vec::with_capacity(11);
        let mut cursor = Some(parent);
        while let Some(e) = cursor {
            times.push(e.header.time);
            if times.len() == 11 {
                break;
            }
            cursor = self.entries.get(&e.header.prev);
        }
        times.sort_unstable();
        times[times.len() / 2]
    }

    /// Validates and adds `header`. Returns whether the best tip changed
    /// (a new tip on the same chain, or a reorg).
    pub fn add(&mut self, header: Header) -> Result<bool, HeaderError> {
        let hash = header.hash();
        if self.entries.contains_key(&hash) {
            return Err(HeaderError::Duplicate);
        }
        let parent = self
            .entries
            .get(&header.prev)
            .ok_or(HeaderError::UnknownParent)?;
        let expected = self.expected_bits(parent)?;
        if header.bits != expected {
            return Err(HeaderError::WrongDifficulty {
                expected,
                found: header.bits,
            });
        }
        if !header.meets_own_target() {
            return Err(HeaderError::InsufficientWork);
        }
        if header.time <= self.median_time_past(parent) {
            return Err(HeaderError::TimeTooOld);
        }
        let work = target_from_compact(header.bits).map_or(U256::ZERO, work_from_target);
        let chainwork = parent.chainwork.checked_add(work).unwrap_or(U256::MAX);
        let height = parent.height + 1;
        self.entries.insert(
            hash,
            Entry {
                header,
                height,
                chainwork,
            },
        );
        // Strictly more work to move: an equal-work sibling never displaces
        // the tip a node already follows, as in Core.
        if chainwork > self.chainwork() {
            self.tip = hash;
            return Ok(true);
        }
        Ok(false)
    }
}
