//! Dispensers that do not touch a chain.
//!
//! Compiled unconditionally rather than behind `#[cfg(test)]`, because the
//! integration tests are separate crates and cannot see a test-only module.
//! They are the reason the [`Dispenser`] trait exists: the properties worth
//! testing here — that a second request from one IP is refused, that a
//! thousand concurrent requests hand out no more than the cap — are properties
//! of the policy, and running them against a real node would test `RocksDB`.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::dispense::{DispenseError, Dispenser};

/// Accepts every send and returns a synthetic transaction id.
#[derive(Debug, Default)]
pub struct NullDispenser;

#[async_trait]
impl Dispenser for NullDispenser {
    async fn send(
        &self,
        recipient: &[u8; 32],
        _amount: primitive_types::U256,
    ) -> Result<String, DispenseError> {
        Ok(hex::encode(recipient))
    }
}

/// Records every send, so a test can count what was actually handed out.
#[derive(Debug, Default)]
pub struct RecordingDispenser {
    sent: Mutex<Vec<([u8; 32], primitive_types::U256)>>,
}

impl RecordingDispenser {
    /// A dispenser with no history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `(recipient, amount)` sent, in order.
    ///
    /// # Panics
    ///
    /// If a test thread panicked while holding the lock, which is a failure
    /// the test should see rather than hide.
    #[must_use]
    pub fn sent(&self) -> Vec<([u8; 32], primitive_types::U256)> {
        self.sent.lock().expect("no test panicked mid-send").clone()
    }

    /// Units dispensed in total.
    ///
    /// The number a load test checks against the cap.
    #[must_use]
    pub fn total(&self) -> primitive_types::U256 {
        self.sent()
            .iter()
            .fold(primitive_types::U256::zero(), |sum, (_, amount)| {
                sum + amount
            })
    }
}

#[async_trait]
impl Dispenser for RecordingDispenser {
    async fn send(
        &self,
        recipient: &[u8; 32],
        amount: primitive_types::U256,
    ) -> Result<String, DispenseError> {
        self.sent
            .lock()
            .expect("no test panicked mid-send")
            .push((*recipient, amount));
        Ok(hex::encode(recipient))
    }
}

/// Fails every send, for the path where a grant is recorded and the node
/// refuses it.
#[derive(Debug, Default)]
pub struct FailingDispenser;

#[async_trait]
impl Dispenser for FailingDispenser {
    async fn send(
        &self,
        _recipient: &[u8; 32],
        _amount: primitive_types::U256,
    ) -> Result<String, DispenseError> {
        Err(DispenseError::Node("the node is down".to_string()))
    }
}
