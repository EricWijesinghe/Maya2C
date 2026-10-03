//! What the watcher needs from a chain, and nothing more.

use async_trait::async_trait;

use custom_l1_node::rpc::HtlcLockInfo;
use maya_htlc_lattice::{Address, CommitmentId, LockRecord, Settlement, Unlock};

use crate::error::{Result, WatcherError};
use crate::swap::LockId;

/// A lock's settlement as the watcher sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockState {
    /// Escrowed.
    Locked,
    /// Claimed at `height`, publishing `unlock`.
    Claimed {
        /// Height of the claiming block.
        height: u64,
        /// The published preimage or opening.
        unlock: Unlock,
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
    /// The lock's id (`maya_htlc_lattice::Lock::id`).
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
            Settlement::Claimed { height, unlock } => LockState::Claimed {
                height: *height,
                unlock: unlock.clone(),
            },
            Settlement::Refunded { height } => LockState::Refunded { height: *height },
        };
        Self {
            sender: record.sender,
            recipient: record.recipient,
            amount: record.amount,
            expiry_height: record.expiry_height,
            commitment_id: record.lock.id(),
            state,
        }
    }

    /// A view of an RPC report. The unlock is decoded and so bound-checked
    /// here: a node that reported an out-of-bound "opening" or a short
    /// preimage gets an error, not a claim transaction the chain would ignore.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Rpc`] for malformed hex, an unknown status, or a claimed
    /// lock without a decodable unlock.
    pub fn from_info(info: &HtlcLockInfo) -> Result<Self> {
        let state = match (info.status.as_str(), info.settled_height, &info.unlock) {
            ("locked", _, _) => LockState::Locked,
            ("claimed", Some(height), Some(unlock)) => {
                let bytes = hex_bytes(unlock, "unlock")?;
                let (unlock, used) = Unlock::decode(&bytes)
                    .map_err(|e| WatcherError::Rpc(format!("unlock: {e}")))?;
                if used != bytes.len() {
                    return Err(WatcherError::Rpc("unlock has trailing bytes".to_owned()));
                }
                LockState::Claimed { height, unlock }
            }
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

    /// The chain's fee terms, if it charges fees (ADR-029): the base fee per
    /// byte and the collector every fee output pays. `None` where fees are
    /// off, which is the default for a chain that never says otherwise.
    async fn fees(&self) -> Result<Option<Fees>> {
        Ok(None)
    }

    /// An account's `(balance, nonce)`, or `None` where the chain does not
    /// say — in which case no fee allowance is paid.
    async fn account(&self, _address: &Address) -> Result<Option<(u64, u64)>> {
        Ok(None)
    }

    /// The chain's genesis block id, used as the chain tag for signatures (ADR-036).
    async fn chain_tag(&self) -> Result<custom_l1_node::core::ChainTag>;
}

/// A chain's fee terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fees {
    /// Base fee per transaction byte.
    pub base_fee: u64,
    /// The address fee outputs pay.
    pub collector: Address,
}

fn hex_bytes(text: &str, what: &str) -> Result<Vec<u8>> {
    hex::decode(text).map_err(|e| WatcherError::Rpc(format!("{what} is not hex: {e}")))
}

fn hex_array(text: &str, what: &str) -> Result<[u8; 32]> {
    hex_bytes(text, what)?
        .try_into()
        .map_err(|_| WatcherError::Rpc(format!("{what} is not 32 bytes")))
}
