//! Which epoch's cache a block is checked against, and how many of them a node
//! keeps.
//!
//! ## Why more than one
//!
//! A block is validated against the cache for *its own* epoch, and a reorg can
//! cross an epoch boundary: a chain that reorganises from height 30,001 back to
//! 29,998 and forward down another branch needs epoch 0 and epoch 1 within a
//! few milliseconds of each other. Holding one cache would mean regenerating —
//! seconds of CPU — inside fork choice, every time a boundary was recrossed,
//! which is a denial of service a peer could trigger deliberately.
//!
//! Two is the whole requirement and the whole policy: the current epoch and the
//! one before it. At [`Params::MAINNET`] that is 128 MiB, held by every
//! validating node.
//!
//! ## Why generation happens under the lock
//!
//! [`CacheRegistry::cache_for_epoch`] holds its mutex across generation, so a
//! second thread asking for the same epoch waits rather than building a second
//! copy of the same 64 MiB array. The wait is bounded by one generation — about
//! a second — and only ever happens on the first block of an epoch, because a
//! node that follows the chain has already pulled the next epoch in through
//! [`CacheRegistry::prepare`]. The alternative, dropping the lock and letting
//! both threads build, spends 64 MiB and a second of CPU to avoid a wait of the
//! same length.

use std::sync::{Arc, Mutex};

use crate::consensus::difficulty::DEFAULT_POW_LIMIT;
use crate::crypto::dag::cache::Cache;
use crate::crypto::dag::{DAG_ACTIVATION_HEIGHT, Params};
use crate::error::{NodeError, Result};

/// Epoch caches retained: the current one and its predecessor.
const RETAINED_EPOCHS: usize = 2;

/// Four leading zero bits: the fork target [`DagConfig::TESTING`] uses.
const TESTING_ACTIVATION_TARGET: [u8; 32] = {
    let mut target = [0xFFu8; 32];
    target[0] = 0x0F;
    target
};

/// Which DAG a chain runs, and from which height.
///
/// `Copy`, so it sits inside `ChainConfig` without changing that type's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DagConfig {
    /// Cache and dataset sizes.
    pub params: Params,
    /// First height whose proof of work is the DAG rather than ArgonBlake.
    pub activation_height: u64,
    /// The target the fork block is mined to, replacing the inherited one.
    ///
    /// ## Why difficulty has to be reset here
    ///
    /// The target in force just before the fork was calibrated against
    /// ArgonBlake: one hash, 32 MiB of Argon2id, ~25 ms of CPU. The rule that
    /// replaces it costs about a millisecond on the same CPU and microseconds
    /// on a GPU. Carrying the old target across would let the next retarget
    /// window be mined in seconds, and retargeting moves by at most
    /// `MAX_ADJUSTMENT_FACTOR` per window, so the chain would sprint for a long
    /// time before it settled.
    ///
    /// ## Why the default is the *easiest* permitted target
    ///
    /// Nobody knows the network's DAG hash rate before the fork, and the two
    /// ways of being wrong are not symmetric. Too easy means blocks arrive too
    /// fast for a few windows, which retargeting fixes on its own. Too hard
    /// means the chain stops, and a stopped chain cannot retarget its way out —
    /// it needs a coordinated intervention. So the default starts at the floor
    /// and lets difficulty be discovered upward.
    pub activation_target: [u8; 32],
}

impl DagConfig {
    /// Consensus configuration: mainnet sizes, forking at
    /// [`DAG_ACTIVATION_HEIGHT`] at the network's difficulty floor.
    pub const MAINNET: Self = Self {
        params: Params::MAINNET,
        activation_height: DAG_ACTIVATION_HEIGHT,
        activation_target: DEFAULT_POW_LIMIT,
    };

    /// Test configuration: small sizes, forking at height 1.
    ///
    /// Height 1 rather than 0 because genesis is never validated — it defines
    /// the starting state rather than transitioning into it — so a fork at 0
    /// and a fork at 1 are the same chain, and 1 is the honest description.
    ///
    /// The activation target is four leading zero bits: one solution in
    /// sixteen, which a test can actually grind.
    pub const TESTING: Self = Self {
        params: Params::TESTING,
        activation_height: 1,
        activation_target: TESTING_ACTIVATION_TARGET,
    };

    /// A configuration that never activates the DAG.
    ///
    /// The pre-fork rule, for tests and tooling that want ArgonBlake at every
    /// height without knowing what the activation height happens to be.
    pub const NEVER: Self = Self {
        params: Params::MAINNET,
        activation_height: u64::MAX,
        activation_target: DEFAULT_POW_LIMIT,
    };

    /// Whether a block at `height` is checked with the DAG.
    #[must_use]
    pub fn is_active(&self, height: u64) -> bool {
        height >= self.activation_height
    }

    /// The epoch a block at `height` belongs to.
    #[must_use]
    pub fn epoch_of(&self, height: u64) -> u64 {
        self.params.epoch_of(height)
    }
}

/// The epoch caches a node holds, generated on demand and reused.
pub struct CacheRegistry {
    config: DagConfig,
    /// Most recently used first. At most [`RETAINED_EPOCHS`] entries.
    entries: Mutex<Vec<Arc<Cache>>>,
}

impl CacheRegistry {
    /// Creates an empty registry. Nothing is generated until it is asked for,
    /// so a chain that never reaches the activation height never allocates.
    #[must_use]
    pub fn new(config: DagConfig) -> Self {
        Self {
            config,
            entries: Mutex::new(Vec::new()),
        }
    }

    /// The configuration this registry serves.
    #[must_use]
    pub fn config(&self) -> DagConfig {
        self.config
    }

    /// Whether a block at `height` is checked with the DAG.
    #[must_use]
    pub fn is_active(&self, height: u64) -> bool {
        self.config.is_active(height)
    }

    /// The cache for `epoch`, generating it if it is not already held.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::DagCacheUnavailable`] if another thread panicked
    /// while holding the registry, or propagates
    /// [`NodeError::DagAllocation`] if the cache cannot be allocated.
    pub fn cache_for_epoch(&self, epoch: u64) -> Result<Arc<Cache>> {
        let mut entries = self.entries.lock().map_err(|_| {
            // A poisoned lock means a thread panicked mid-generation. The held
            // caches are still sound — they are immutable once built — but
            // reporting is better than papering over a panic that already
            // happened somewhere else.
            NodeError::DagCacheUnavailable {
                epoch,
                reason: "the cache registry was poisoned by a panicking thread".to_string(),
            }
        })?;

        if let Some(position) = entries.iter().position(|cache| cache.epoch() == epoch) {
            // Move to front: the two epochs in play during a boundary reorg
            // must both survive eviction.
            let cache = entries.remove(position);
            entries.insert(0, Arc::clone(&cache));
            return Ok(cache);
        }

        let cache = Arc::new(Cache::generate(epoch, self.config.params)?);
        entries.insert(0, Arc::clone(&cache));
        entries.truncate(RETAINED_EPOCHS);
        Ok(cache)
    }

    /// The cache a block at `height` is validated against.
    ///
    /// # Errors
    ///
    /// As [`CacheRegistry::cache_for_epoch`].
    pub fn cache_for_height(&self, height: u64) -> Result<Arc<Cache>> {
        self.cache_for_epoch(self.config.epoch_of(height))
    }

    /// Generates an epoch's cache ahead of needing it.
    ///
    /// A node that calls this as it approaches a boundary never pays generation
    /// inside block validation, which is the difference between a smooth epoch
    /// change and a node that stops relaying for a second every 5.21 days.
    ///
    /// # Errors
    ///
    /// As [`CacheRegistry::cache_for_epoch`].
    pub fn prepare(&self, epoch: u64) -> Result<()> {
        self.cache_for_epoch(epoch).map(|_| ())
    }

    /// Epochs currently held, most recently used first.
    ///
    /// Exists for the tests that assert the retention policy, and for a node
    /// that wants to report what it is holding.
    #[must_use]
    pub fn held_epochs(&self) -> Vec<u64> {
        self.entries
            .lock()
            .map(|entries| entries.iter().map(|cache| cache.epoch()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn registry() -> CacheRegistry {
        CacheRegistry::new(DagConfig::TESTING)
    }

    #[test]
    fn a_fresh_registry_holds_nothing() {
        // Lazily populated on purpose: a chain below the activation height must
        // not allocate 64 MiB for a rule it never applies.
        assert!(registry().held_epochs().is_empty());
    }

    #[test]
    fn a_cache_is_generated_once_and_then_reused() {
        let registry = registry();
        let first = registry
            .cache_for_epoch(0)
            .expect("generation must succeed");
        let second = registry
            .cache_for_epoch(0)
            .expect("generation must succeed");

        assert!(
            Arc::ptr_eq(&first, &second),
            "the second request rebuilt the cache instead of reusing it"
        );
        assert_eq!(registry.held_epochs(), vec![0]);
    }

    #[test]
    fn both_sides_of_an_epoch_boundary_stay_resident() {
        // The reorg case. If asking for epoch 1 evicted epoch 0, a fork choice
        // that crossed the boundary twice would regenerate on every crossing.
        let registry = registry();
        let zero = registry
            .cache_for_epoch(0)
            .expect("generation must succeed");
        let _ = registry
            .cache_for_epoch(1)
            .expect("generation must succeed");

        assert_eq!(registry.held_epochs(), vec![1, 0]);

        let zero_again = registry
            .cache_for_epoch(0)
            .expect("generation must succeed");
        assert!(Arc::ptr_eq(&zero, &zero_again), "epoch 0 was evicted");
        assert_eq!(registry.held_epochs(), vec![0, 1]);
    }

    #[test]
    fn a_third_epoch_evicts_the_least_recently_used() {
        let registry = registry();
        for epoch in 0..3 {
            let _ = registry
                .cache_for_epoch(epoch)
                .expect("generation must succeed");
        }

        assert_eq!(registry.held_epochs(), vec![2, 1]);
    }

    #[test]
    fn height_selects_the_epoch_the_parameters_define() {
        let registry = registry();
        let length = DagConfig::TESTING.params.epoch_length;

        let first = registry
            .cache_for_height(length - 1)
            .expect("generation must succeed");
        let second = registry
            .cache_for_height(length)
            .expect("generation must succeed");

        assert_eq!(first.epoch(), 0);
        assert_eq!(second.epoch(), 1);
    }

    #[test]
    fn preparation_leaves_the_cache_ready() {
        let registry = registry();
        registry.prepare(1).expect("generation must succeed");
        assert_eq!(registry.held_epochs(), vec![1]);
    }

    #[test]
    fn activation_is_a_height_threshold() {
        let mainnet = DagConfig::MAINNET;
        assert!(!mainnet.is_active(DAG_ACTIVATION_HEIGHT - 1));
        assert!(mainnet.is_active(DAG_ACTIVATION_HEIGHT));
        assert!(mainnet.is_active(u64::MAX));

        // The escape hatch used by tooling that wants the pre-fork rule.
        assert!(!DagConfig::NEVER.is_active(u64::MAX - 1));
    }

    #[test]
    fn generated_caches_carry_the_configured_parameters() {
        let registry = registry();
        let cache = registry
            .cache_for_epoch(0)
            .expect("generation must succeed");
        assert_eq!(cache.params(), Params::TESTING);
    }
}
