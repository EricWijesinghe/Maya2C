//! What the watcher needs from a chain, and nothing more.

use async_trait::async_trait;

use custom_l1_node::rpc::HtlcLockInfo;
use maya_htlc_lattice::{Address, CommitmentId, LockRecord, Opening, Settlement};

use crate::error::{Result, WatcherError};
use crate::swap::LockId;

/// A lock's settlement as the watcher sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockState {
    /// Escrowed.
    Locked,
    /// Claimed at `height`, publishing `opening`.
    Claimed {
        /// Height of the claiming block.
        height: u64,
        /// The published opening.
        opening: Opening,
    },
    /// Refunded at `height`.
    Refunded {
        /// Height of the refunding block.
        height: u64,
    },
}

/// One lock, as a chain reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockView {
    /// Who funded it.
    pub sender: Address,
    /// Who a claim pays.
    pub recipient: Address,
    /// Escrowed base units.
    pub amount: u64,
    /// First height at which a claim is refused.
    pub expiry_height: u64,
    /// The commitment's id.
    pub commitment_id: CommitmentId,
    /// Where it stands.
    pub state: LockState,
}

impl LockView {
    /// A view of a stored record.
    #[must_use]
    pub fn from_record(record: &LockRecord) -> Self {
        let state = match &record.settlement {
            Settlement::Open => LockState::Locked,
            Settlement::Claimed { height, opening } => LockState::Claimed {
                height: *height,
                opening: opening.clone(),
            },
            Settlement::Refunded { height } => LockState::Refunded { height: *height },
        };
        Self {
            sender: record.sender,
            recipient: record.recipient,
            amount: record.amount,
            expiry_height: record.expiry_height,
            commitment_id: record.commitment.id(),
            state,
        }
    }

    /// A view of an RPC report. The opening is decoded and so bound-checked
    /// here: a node that reported an out-of-bound "opening" gets an error, not
    /// a claim transaction the chain would ignore.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Rpc`] for malformed hex, an unknown status, or a claimed
    /// lock without a decodable opening.
    pub fn from_info(info: &HtlcLockInfo) -> Result<Self> {
        let state = match (info.status.as_str(), info.settled_height, &info.opening) {
            ("locked", _, _) => LockState::Locked,
            ("claimed", Some(height), Some(opening)) => LockState::Claimed {
                height,
                opening: Opening::decode(&hex_bytes(opening, "opening")?)
                    .map_err(|e| WatcherError::Rpc(format!("opening: {e}")))?,
            },
            ("refunded", Some(height), _) => LockState::Refunded { height },
            (status, _, _) => {
                return Err(WatcherError::Rpc(format!(
                    "lock report with status {status:?} is incomplete"
                )));
            }
        };
        Ok(Self {
            sender: hex_array(&info.sender, "sender")?,
            recipient: hex_array(&info.recipient, "recipient")?,
            amount: info.amount,
            expiry_height: info.expiry_height,
            commitment_id: hex_array(&info.commitment_id, "commitment_id")?,
            state,
        })
    }

    /// The height it settled at, if it has.
    #[must_use]
    pub const fn settled_height(&self) -> Option<u64> {
        match self.state {
            LockState::Locked => None,
            LockState::Claimed { height, .. } | LockState::Refunded { height } => Some(height),
        }
    }
}

/// A chain the watcher can observe and submit to.
#[async_trait]
pub trait SwapChain: Send + Sync {
    /// Height of the active tip.
    async fn tip_height(&self) -> Result<u64>;

    /// A lock, if the active chain holds it.
    async fn lock(&self, id: &LockId) -> Result<Option<LockView>>;

    /// The nonce the account's next committed transaction must carry.
    async fn next_nonce(&self, address: &Address) -> Result<u64>;

    /// Submits signed transaction bytes. A duplicate is success.
    async fn broadcast(&self, raw: &[u8]) -> Result<()>;
}

fn hex_bytes(text: &str, what: &str) -> Result<Vec<u8>> {
    hex::decode(text).map_err(|e| WatcherError::Rpc(format!("{what} is not hex: {e}")))
}

fn hex_array(text: &str, what: &str) -> Result<[u8; 32]> {
    hex_bytes(text, what)?
        .try_into()
        .map_err(|_| WatcherError::Rpc(format!("{what} is not 32 bytes")))
}
