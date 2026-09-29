//! Checking blocks, not only headers. Research branch.
//!
//! [`LightClient`] follows headers and believes that whatever produced a state
//! root was legal — the SPV trade-off in `lib.rs`. For transfer blocks a
//! witness removes that belief: [`StatelessValidator`] re-runs each block's
//! transfers against its witness and checks the declared root, holding 32 bytes
//! of state between blocks and one block's witness while it works.
//!
//! What it still does not do:
//!
//! - **Proof of work.** That is [`crate::HeaderChain`]'s job, and its DAG cache
//!   is tens of megabytes. [`LightClient::verify_block`] joins the two: a
//!   work-checked header, and a block checked against it.
//! - **Anything but transfers,** or any chain whose oracle, governance or
//!   sealed mempool holds records. Those answer
//!   [`LightClientError::BlockUnverifiable`], which is not a verdict — see
//!   `custom_l1_node::state::stateless`.
//! - **Fetch witnesses.** No gossip topic carries them yet.

use custom_l1_node::core::{BlockHeader, ChainTag};
use custom_l1_node::core::block::Block;
use custom_l1_node::state::{StateWitness, StatelessError, verify_block};

use crate::chain::LightClient;
use crate::error::{LightClientError, Result};

/// A node that validates a chain of transfer blocks from witnesses alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatelessValidator {
    tip_id: [u8; 32],
    state_root: [u8; 32],
    height: u64,
}

impl StatelessValidator {
    /// Starts from a header the caller already trusts, at `height`.
    #[must_use]
    pub fn new(anchor: &BlockHeader, height: u64) -> Self {
        Self {
            tip_id: anchor.id(),
            state_root: anchor.state_root,
            height,
        }
    }

    /// Height of the last block validated.
    #[must_use]
    pub const fn height(&self) -> u64 {
        self.height
    }

    /// State root after the last block validated.
    #[must_use]
    pub const fn state_root(&self) -> [u8; 32] {
        self.state_root
    }

    /// Validates the next block and advances to it.
    ///
    /// # Errors
    ///
    /// - [`LightClientError::UnknownParent`] if the block does not extend the
    ///   tip.
    /// - [`LightClientError::BlockInvalid`] if the block breaks a rule.
    /// - [`LightClientError::BlockUnverifiable`] if this witness and body
    ///   cannot decide it. The tip does not move in either failure case.
    pub fn validate(&mut self, block: &Block, witness: StateWitness) -> Result<()> {
        let height = self
            .height
            .checked_add(1)
            .ok_or(LightClientError::UnknownHeight(self.height))?;
        if block.header.prev_hash != self.tip_id {
            return Err(LightClientError::UnknownParent {
                header: hex::encode(block.header.id()),
                parent: hex::encode(block.header.prev_hash),
            });
        }
        verify_block(&self.state_root, height, block, witness)
            .map_err(|error| verdict(height, error))?;
        self.tip_id = block.header.id();
        self.state_root = block.header.state_root;
        self.height = height;
        Ok(())
    }
}

impl LightClient {
    /// Verifies the block at `height` of the most-work chain from `witness`.
    ///
    /// # Errors
    ///
    /// - [`LightClientError::UnknownHeight`] if the client holds no header at
    ///   `height` or at its parent.
    /// - [`LightClientError::NotBest`] if `block` is not the header held there.
    /// - [`LightClientError::BlockInvalid`] or
    ///   [`LightClientError::BlockUnverifiable`], as
    ///   [`StatelessValidator::validate`].
    pub fn verify_block(&self, height: u64, block: &Block, witness: StateWitness) -> Result<()> {
        let record = self
            .chain()
            .header_at(height)
            .ok_or(LightClientError::UnknownHeight(height))?;
        if record.header.id() != block.header.id() {
            return Err(LightClientError::NotBest(hex::encode(block.header.id())));
        }
        let parent_height = height
            .checked_sub(1)
            .ok_or(LightClientError::UnknownHeight(height))?;
        let parent = self
            .chain()
            .header_at(parent_height)
            .ok_or(LightClientError::UnknownHeight(parent_height))?;
        verify_block(&parent.header.state_root, height, block, witness)
            .map_err(|error| verdict(height, error))
    }
}

fn verdict(height: u64, error: StatelessError) -> LightClientError {
    match error {
        StatelessError::Invalid(reason) => LightClientError::BlockInvalid { height, reason },
        StatelessError::Unverifiable(reason) => {
            LightClientError::BlockUnverifiable { height, reason }
        }
    }
}
