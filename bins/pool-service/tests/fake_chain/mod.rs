//! A chain the integration tests drive by hand.
//!
//! Implements [`ChainView`] — the four things the payout engine needs from the
//! chain — over a `Mutex`ed struct the test can poke. The engine's own unit
//! tests carry a private copy of this shape; this one exists so the
//! session-level tests can share it without making the private one public and
//! turning a test fixture into part of the crate's API.

use std::sync::Mutex;

use custom_l1_node::state::Address;
use maya_pool_service::error::Result;
use maya_pool_service::payout::ChainView;

/// Mutable chain state.
#[derive(Default)]
struct FakeState {
    /// Active tip height.
    height: u64,
    /// Block id at each height.
    blocks: Vec<(u64, String)>,
    /// Treasury balance.
    balance: u64,
    /// Treasury nonce.
    nonce: u64,
    /// Raw transactions broadcast, in order.
    broadcasts: Vec<Vec<u8>>,
}

/// A chain the tests control.
#[derive(Default)]
pub struct FakeChain {
    /// See [`FakeState`].
    state: Mutex<FakeState>,
}

impl FakeChain {
    /// A chain at `height` with a treasury holding `balance`.
    #[must_use]
    pub fn with(height: u64, balance: u64) -> Self {
        Self {
            state: Mutex::new(FakeState {
                height,
                balance,
                ..FakeState::default()
            }),
        }
    }

    /// Places a block id at a height.
    pub fn put_block(&self, height: u64, id: &str) {
        self.state
            .lock()
            .expect("fake chain lock")
            .blocks
            .push((height, id.to_string()));
    }

    /// Moves the tip.
    pub fn set_height(&self, height: u64) {
        self.state.lock().expect("fake chain lock").height = height;
    }

    /// Transactions broadcast so far.
    #[must_use]
    pub fn broadcasts(&self) -> Vec<Vec<u8>> {
        self.state
            .lock()
            .expect("fake chain lock")
            .broadcasts
            .clone()
    }

    /// Total value carried by every broadcast payout.
    ///
    /// Decoded from the transactions themselves rather than tracked alongside
    /// them: the point of the assertion this feeds is that what left the
    /// treasury matches what the ledger says, and a counter incremented by the
    /// test would be measuring the test.
    #[must_use]
    pub fn broadcast_value(&self) -> u64 {
        self.broadcasts()
            .iter()
            .filter_map(|raw| custom_l1_node::core::Transaction::from_bytes(raw).ok())
            .flat_map(|transaction| {
                transaction
                    .outputs
                    .into_iter()
                    .map(|output| output.amount)
                    .collect::<Vec<_>>()
            })
            .fold(0u64, u64::saturating_add)
    }
}

#[async_trait::async_trait]
impl ChainView for FakeChain {
    async fn chain_height(&self) -> Result<u64> {
        Ok(self.state.lock().expect("fake chain lock").height)
    }

    async fn block_id_at(&self, height: u64) -> Result<Option<String>> {
        Ok(self
            .state
            .lock()
            .expect("fake chain lock")
            .blocks
            .iter()
            .find(|(stored, _)| *stored == height)
            .map(|(_, id)| id.clone()))
    }

    async fn account(&self, _address: &Address) -> Result<(u64, u64)> {
        let state = self.state.lock().expect("fake chain lock");
        Ok((state.balance, state.nonce))
    }

    async fn broadcast(&self, raw: &[u8]) -> Result<String> {
        let mut state = self.state.lock().expect("fake chain lock");
        state.broadcasts.push(raw.to_vec());
        // The nonce advances exactly as the real chain's would, which is the
        // signal the payout watcher reads to decide a batch landed.
        state.nonce += 1;
        Ok(format!("tx{}", state.broadcasts.len()))
    }
}
