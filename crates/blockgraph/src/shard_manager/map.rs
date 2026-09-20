//! A tiling of the address space by binary prefixes.
//!
//! A leaf is a prefix of the address's first 32 bits: `depth` leading bits
//! fixed, the rest free. Leaves are held in key order and must cover the space
//! exactly once, so a leaf is a contiguous key range — a RocksDB range scan, as
//! for the fixed partition — and lookup is a binary search.
//!
//! Merges are **buddy** merges only: two leaves merge exactly when they are the
//! two halves of one parent. So every map is reachable from the root by splits
//! alone, and there is one map for any set of leaves rather than several that
//! cover the same space differently.

use alloc::vec::Vec;

use crate::error::{GraphError, Result};
use crate::schedule::Access;
use crate::shard::{SHARD_COUNT, ShardId};

/// Deepest a leaf may go: the address bits a prefix can fix.
///
/// Addresses are BLAKE3 digests, so pushing activity into one 32-bit prefix
/// means generating around 2^32 keypairs per address. A hot range deeper than
/// that is not load; it is somebody grinding.
pub const MAX_PREFIX_BITS: u8 = 32;

/// One past the largest key: the size of the space a map must tile.
const KEYSPACE: u64 = 1 << MAX_PREFIX_BITS;

/// The 32 key bits a prefix is taken from: the address's first four bytes.
#[must_use]
pub const fn key_bits(address: &[u8; 32]) -> u32 {
    u32::from_be_bytes([address[0], address[1], address[2], address[3]])
}

/// A key range: the addresses whose first `depth` bits equal those of `bits`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Prefix {
    bits: u32,
    depth: u8,
}

impl Prefix {
    /// The whole address space.
    pub const ROOT: Self = Self { bits: 0, depth: 0 };

    /// The prefix fixing the first `depth` bits of `bits`.
    ///
    /// # Errors
    ///
    /// [`GraphError::PrefixTooDeep`] past [`MAX_PREFIX_BITS`];
    /// [`GraphError::NonCanonicalPrefix`] if `bits` sets a bit below `depth`,
    /// which would be a second spelling of the same range.
    pub const fn new(bits: u32, depth: u8) -> Result<Self> {
        if depth > MAX_PREFIX_BITS {
            return Err(GraphError::PrefixTooDeep);
        }
        let span = 1u64 << (MAX_PREFIX_BITS - depth);
        if (bits as u64) & (span - 1) != 0 {
            return Err(GraphError::NonCanonicalPrefix);
        }
        Ok(Self { bits, depth })
    }

    /// The fixed bits, left-aligned.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.bits
    }

    /// How many leading bits are fixed.
    #[must_use]
    pub const fn depth(self) -> u8 {
        self.depth
    }

    /// First key in the range.
    #[must_use]
    pub const fn start(self) -> u64 {
        self.bits as u64
    }

    /// Keys in the range.
    #[must_use]
    pub const fn span(self) -> u64 {
        1u64 << (MAX_PREFIX_BITS - self.depth)
    }

    /// Whether `address` is in the range.
    #[must_use]
    pub const fn contains(self, address: &[u8; 32]) -> bool {
        let key = key_bits(address) as u64;
        key >= self.start() && key - self.start() < self.span()
    }

    /// The two halves: next bit 0, then next bit 1.
    ///
    /// # Errors
    ///
    /// [`GraphError::PrefixTooDeep`] at [`MAX_PREFIX_BITS`].
    pub const fn children(self) -> Result<(Self, Self)> {
        if self.depth >= MAX_PREFIX_BITS {
            return Err(GraphError::PrefixTooDeep);
        }
        let depth = self.depth + 1;
        // At most 2^31 below the root, so the cast is exact.
        let half = (self.span() / 2) as u32;
        Ok((
            Self {
                bits: self.bits,
                depth,
            },
            Self {
                bits: self.bits + half,
                depth,
            },
        ))
    }

    /// The range this is one half of, `None` for the root.
    #[must_use]
    pub const fn parent(self) -> Option<Self> {
        if self.depth == 0 {
            return None;
        }
        let depth = self.depth - 1;
        let span = 1u64 << (MAX_PREFIX_BITS - depth);
        Some(Self {
            bits: ((self.bits as u64) & !(span - 1)) as u32,
            depth,
        })
    }

    /// Which half of this range `address` is in — `true` for the upper — or
    /// `None` at [`MAX_PREFIX_BITS`], where there are no halves.
    #[must_use]
    pub const fn half_of(self, address: &[u8; 32]) -> Option<bool> {
        if self.depth >= MAX_PREFIX_BITS {
            return None;
        }
        Some((key_bits(address) >> (31 - self.depth as u32)) & 1 == 1)
    }
}

/// The leaves a node schedules over, in key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardMap {
    leaves: Vec<Prefix>,
}

impl ShardMap {
    /// `2^depth` equal leaves.
    ///
    /// # Errors
    ///
    /// [`GraphError::ShardLimit`] for more than [`SHARD_COUNT`] leaves.
    pub fn uniform(depth: u8) -> Result<Self> {
        if depth > SHARD_COUNT.trailing_zeros() as u8 {
            return Err(GraphError::ShardLimit);
        }
        let span = 1u64 << (MAX_PREFIX_BITS - depth);
        let leaves = (0..1u64 << depth)
            .map(|index| Prefix {
                bits: (index * span) as u32,
                depth,
            })
            .collect();
        Ok(Self { leaves })
    }

    /// A map from explicit leaves.
    ///
    /// # Errors
    ///
    /// [`GraphError::ShardLimit`] for more than [`SHARD_COUNT`] leaves — an
    /// access set is a `u64`; [`GraphError::NotATiling`] unless the leaves are
    /// in key order and cover the space exactly once.
    pub fn from_leaves(leaves: Vec<Prefix>) -> Result<Self> {
        if leaves.len() > SHARD_COUNT {
            return Err(GraphError::ShardLimit);
        }
        let mut expected = 0u64;
        for leaf in &leaves {
            if leaf.start() != expected {
                return Err(GraphError::NotATiling);
            }
            expected += leaf.span();
        }
        if expected != KEYSPACE {
            return Err(GraphError::NotATiling);
        }
        Ok(Self { leaves })
    }

    /// How many leaves.
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.leaves.len()
    }

    /// The leaves, in key order. Leaf `i` is shard `i`.
    #[must_use]
    pub fn leaves(&self) -> &[Prefix] {
        &self.leaves
    }

    /// The range of `shard`, if the map has one.
    #[must_use]
    pub fn prefix(&self, shard: ShardId) -> Option<Prefix> {
        self.leaves.get(shard.index()).copied()
    }

    /// The leaf holding `address`.
    #[must_use]
    pub fn locate(&self, address: &[u8; 32]) -> ShardId {
        let key = u64::from(key_bits(address));
        // The first leaf starts at 0, so at least one leaf starts at or below
        // any key and the subtraction never saturates in a valid map.
        let index = self
            .leaves
            .partition_point(|leaf| leaf.start() <= key)
            .saturating_sub(1);
        ShardId::from_leaf(index)
    }

    /// The access set of a transaction naming `addresses`, under this map.
    ///
    /// # Errors
    ///
    /// [`GraphError::EmptyAccessSet`] for no addresses.
    pub fn access_for(&self, addresses: &[[u8; 32]]) -> Result<Access> {
        let mask = addresses.iter().fold(0u64, |mask, address| {
            mask | 1u64 << self.locate(address).index()
        });
        Access::from_mask(mask)
    }

    /// The map with `shard` bisected.
    ///
    /// # Errors
    ///
    /// [`GraphError::UnknownShard`], [`GraphError::PrefixTooDeep`], or
    /// [`GraphError::ShardLimit`].
    pub fn split(&self, shard: ShardId) -> Result<Self> {
        let leaf = self.prefix(shard).ok_or(GraphError::UnknownShard)?;
        let (low, high) = leaf.children()?;
        let mut leaves = Vec::with_capacity(self.leaves.len() + 1);
        leaves.extend_from_slice(&self.leaves[..shard.index()]);
        leaves.push(low);
        leaves.push(high);
        leaves.extend_from_slice(&self.leaves[shard.index() + 1..]);
        Self::from_leaves(leaves)
    }

    /// The map with `shard` and the leaf after it merged into their parent.
    ///
    /// # Errors
    ///
    /// [`GraphError::UnknownShard`], or [`GraphError::NotSiblings`] unless the
    /// two are the halves of one range.
    pub fn merge(&self, shard: ShardId) -> Result<Self> {
        let left = self.prefix(shard).ok_or(GraphError::UnknownShard)?;
        let right = self
            .leaves
            .get(shard.index() + 1)
            .copied()
            .ok_or(GraphError::NotSiblings)?;
        let parent = left.parent().ok_or(GraphError::NotSiblings)?;
        if parent.children()? != (left, right) {
            return Err(GraphError::NotSiblings);
        }
        let mut leaves = Vec::with_capacity(self.leaves.len() - 1);
        leaves.extend_from_slice(&self.leaves[..shard.index()]);
        leaves.push(parent);
        leaves.extend_from_slice(&self.leaves[shard.index() + 2..]);
        Self::from_leaves(leaves)
    }

    /// Every adjacent pair that could merge: the left leaf and the parent.
    ///
    /// A leaf has one sibling, so no leaf appears in two pairs.
    #[must_use]
    pub fn buddies(&self) -> Vec<(ShardId, Prefix)> {
        self.leaves
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| {
                let parent = pair[0].parent()?;
                match parent.children() {
                    Ok((low, high)) if low == pair[0] && high == pair[1] => {
                        Some((ShardId::from_leaf(index), parent))
                    }
                    _ => None,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(first: u32) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..4].copy_from_slice(&first.to_be_bytes());
        out
    }

    fn id(index: usize) -> ShardId {
        ShardId::new(index).expect("in range")
    }

    #[test]
    fn a_uniform_map_tiles_the_space() {
        let map = ShardMap::uniform(2).expect("four");
        assert_eq!(map.shard_count(), 4);
        assert_eq!(ShardMap::from_leaves(map.leaves().to_vec()), Ok(map));
        assert_eq!(ShardMap::uniform(7), Err(GraphError::ShardLimit));
    }

    #[test]
    fn gaps_overlaps_disorder_and_excess_are_refused() {
        let quarter = |bits| Prefix::new(bits, 2).expect("canonical");
        let half = Prefix::new(0, 1).expect("canonical");
        let gap = [quarter(0), quarter(0x8000_0000), quarter(0xC000_0000)];
        let overlap = [
            half,
            quarter(0x4000_0000),
            quarter(0x8000_0000),
            quarter(0xC000_0000),
        ];
        let disorder = [
            quarter(0x4000_0000),
            quarter(0),
            quarter(0x8000_0000),
            quarter(0xC000_0000),
        ];
        for leaves in [&gap[..], &overlap[..], &disorder[..], &[][..]] {
            assert_eq!(
                ShardMap::from_leaves(leaves.to_vec()),
                Err(GraphError::NotATiling)
            );
        }
        let mut deep = ShardMap::uniform(6).expect("64");
        let excess = deep.split(id(0));
        assert_eq!(excess, Err(GraphError::ShardLimit));
        deep = ShardMap::uniform(5).expect("32");
        assert!(deep.split(id(0)).is_ok());
    }

    #[test]
    fn a_prefix_with_bits_below_its_depth_is_refused() {
        assert_eq!(Prefix::new(1, 31), Err(GraphError::NonCanonicalPrefix));
        assert_eq!(Prefix::new(0, 33), Err(GraphError::PrefixTooDeep));
        assert!(Prefix::new(u32::MAX, 32).is_ok());
    }

    #[test]
    fn locate_names_the_one_leaf_containing_the_address() {
        let map = ShardMap::uniform(3)
            .and_then(|map| map.split(id(5)))
            .and_then(|map| map.split(id(5)))
            .expect("map");
        for step in 0..4_096u32 {
            let target = address(step.wrapping_mul(0x9E37_79B9));
            let shard = map.locate(&target);
            let containing: Vec<_> = map
                .leaves()
                .iter()
                .filter(|leaf| leaf.contains(&target))
                .collect();
            assert_eq!(containing.len(), 1);
            assert_eq!(map.prefix(shard).as_ref(), containing.first().copied());
        }
    }

    #[test]
    fn splitting_then_merging_restores_the_map_and_only_buddies_merge() {
        let map = ShardMap::uniform(2).expect("four");
        let split = map.split(id(1)).expect("split");
        assert_eq!(split.shard_count(), 5);
        assert_eq!(split.merge(id(1)), Ok(map.clone()));
        // Leaves 01 and 10 are adjacent but halves of different parents.
        assert_eq!(map.merge(id(1)), Err(GraphError::NotSiblings));
        assert_eq!(map.merge(id(3)), Err(GraphError::NotSiblings));
        assert_eq!(map.buddies().len(), 2);
    }

    #[test]
    fn children_are_the_two_halves_of_their_parent() {
        let prefix = Prefix::new(0xA000_0000, 4).expect("canonical");
        let (low, high) = prefix.children().expect("children");
        assert_eq!((low.parent(), high.parent()), (Some(prefix), Some(prefix)));
        assert_eq!(low.span() + high.span(), prefix.span());
        assert_eq!(high.start(), low.start() + low.span());
        assert_eq!(prefix.half_of(&address(0xA800_0000)), Some(true));
        assert_eq!(prefix.half_of(&address(0xA000_0001)), Some(false));
        let deepest = Prefix::new(7, 32).expect("canonical");
        assert_eq!(deepest.children(), Err(GraphError::PrefixTooDeep));
        assert_eq!(deepest.half_of(&address(7)), None);
        assert_eq!(Prefix::ROOT.parent(), None);
    }
}
