//! Reusing a block's preview when the same node then applies it (Master
//! Prompt 12).
//!
//! A node that builds a block stages it twice: once for the state root its
//! header must carry (`preview_root`), and again moments later when it inserts
//! the block (`stage_checked`). `reports/04-consensus.md` measured block
//! building and insertion at two thirds of wall time in `bft_tps`, with every
//! transaction staged three times and the root computed twice.
//!
//! Staging is a pure function of committed state, the block and its context.
//! So the preview's overlay and root are kept, and the apply takes them
//! instead of recomputing — **only** when all three are provably the same:
//!
//! - the block, by a hash of its header with the state root zeroed (which
//!   binds `prev_hash`, timestamp, nonce, target and `tx_root`) and of every
//!   transaction id;
//! - the context, compared whole;
//! - committed state, by a generation counter every write in `state::db`
//!   advances *before* it writes, read before the preview staged.
//!
//! The header's root is still compared with the kept root, so invariant 24's
//! check runs on every apply. Process-local; nothing here reaches consensus.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::Block;
use crate::state::context::BlockContext;
use crate::state::db::Overlay;

/// A kept preview.
pub(crate) struct Preview {
    key: [u8; 32],
    context: BlockContext,
    generation: u64,
    overlay: Overlay,
    root: [u8; 32],
}

/// The cache and the generation counter it is checked against.
#[derive(Default)]
pub(crate) struct PreviewCache {
    /// Held while a preview stages and while a committed write bumps the
    /// generation and writes, so a preview can never straddle a write. The
    /// chain's mutex serialises these today, but that is a fact about the
    /// callers; this makes the cache sound on its own (review, HIGH).
    serial: Mutex<()>,
    generation: AtomicU64,
    kept: Mutex<Option<Preview>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// What identifies a block's execution, whatever state root it declares.
pub(crate) fn preview_key(block: &Block) -> [u8; 32] {
    let mut header = block.header.clone();
    header.state_root = [0; 32];
    let mut hasher = blake3::Hasher::new_derive_key("maya2c preview key v1");
    hasher.update(&header.id());
    for tx in &block.transactions {
        hasher.update(&tx.txid());
    }
    *hasher.finalize().as_bytes()
}

impl PreviewCache {
    /// Applies that reused a preview, and applies that staged afresh.
    pub(crate) fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    /// The current generation: read before staging a preview.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Starts a committed write: bumps the generation and returns a guard the
    /// caller holds until the write has landed.
    pub(crate) fn begin_write(&self) -> std::sync::MutexGuard<'_, ()> {
        let guard = self
            .serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.generation.fetch_add(1, Ordering::SeqCst);
        guard
    }

    /// Held for the whole of a preview's staging.
    pub(crate) fn begin_preview(&self) -> std::sync::MutexGuard<'_, ()> {
        self.serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    fn advance(&self) {
        drop(self.begin_write());
    }

    /// Keeps a preview staged at `generation`, unless state moved meanwhile.
    pub(crate) fn keep(
        &self,
        block: &Block,
        context: BlockContext,
        generation: u64,
        overlay: Overlay,
        root: [u8; 32],
    ) {
        if generation != self.generation() {
            return;
        }
        let preview = Preview {
            key: preview_key(block),
            context,
            generation,
            overlay,
            root,
        };
        // A poisoned lock only means another thread panicked mid-swap; the
        // cache is an optimisation, so recover it rather than propagate.
        *self
            .kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(preview);
    }

    /// The kept overlay and root for exactly this block, context and state.
    pub(crate) fn take(&self, block: &Block, context: BlockContext) -> Option<(Overlay, [u8; 32])> {
        let mut kept = self
            .kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fits = kept.as_ref().is_some_and(|p| {
            p.generation == self.generation() && p.context == context && p.key == preview_key(block)
        });
        if !fits {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.hits.fetch_add(1, Ordering::Relaxed);
        kept.take().map(|p| (p.overlay, p.root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{BlockHeader, Transaction, TxOutput};

    fn block(nonce: u64, root: u8, txs: Vec<Transaction>) -> Block {
        Block::new(
            BlockHeader {
                prev_hash: [1; 32],
                state_root: [root; 32],
                timestamp: 1_790_000_000,
                nonce,
                difficulty_target: [0xFF; 32],
                tx_root: [0; 32],
            },
            txs,
        )
    }

    #[test]
    fn the_key_ignores_the_declared_root_and_nothing_else() {
        let tx = Transaction::new(
            vec![],
            vec![TxOutput {
                amount: 1,
                recipient: [2; 32],
            }],
            0,
        );
        let base = block(7, 0, vec![tx.clone()]);
        assert_eq!(
            preview_key(&base),
            preview_key(&block(7, 9, vec![tx.clone()])),
            "root is what is computed"
        );
        assert_ne!(
            preview_key(&base),
            preview_key(&block(8, 0, vec![tx.clone()])),
            "the seal"
        );
        assert_ne!(
            preview_key(&base),
            preview_key(&block(7, 0, vec![])),
            "the transactions"
        );
    }

    #[test]
    fn a_write_after_the_preview_makes_it_stale() {
        let cache = PreviewCache::default();
        let b = block(1, 0, vec![]);
        let context = BlockContext::at_height(1);
        let generation = cache.generation();
        cache.keep(&b, context, generation, Overlay::default(), [3; 32]);
        cache.advance();
        assert!(cache.take(&b, context).is_none(), "state moved: recompute");

        let generation = cache.generation();
        cache.keep(&b, context, generation, Overlay::default(), [3; 32]);
        assert!(
            cache.take(&b, BlockContext::at_height(2)).is_none(),
            "another context"
        );
        assert_eq!(cache.take(&b, context).map(|(_, r)| r), Some([3; 32]));
        assert!(cache.take(&b, context).is_none(), "taken once");

        // A write *during* the preview's staging: it is never kept.
        let before = cache.generation();
        cache.advance();
        cache.keep(&b, context, before, Overlay::default(), [3; 32]);
        assert!(cache.take(&b, context).is_none());
    }
}
