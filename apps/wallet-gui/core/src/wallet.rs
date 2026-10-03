//! The wallet session: accounts, pending transfers, and broadcast.
//!
//! A [`Wallet`] holds an unlocked seed for the duration of a session. It is the
//! only place key material lives outside the keychain, and it zeroizes on drop.
//!
//! Broadcasting is deliberately separate from signing: [`crate::payment::sign_transfer`]
//! needs no network, so a signing device never has to have one.

use std::collections::BTreeMap;

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

// Re-exported: the desktop app names the chain it signs for without linking the node.
pub use custom_l1_node::core::ChainTag;

use crate::error::{Result, WalletError};
use crate::hd::{self, DerivationPath, SEED_LEN};
use crate::payment::{
    SignedTransfer, check_fee_collector, sign_transfer, sign_transfer_to, transfer_size,
};

/// A derived account.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    /// Index within the wallet.
    pub index: u32,
    /// Derivation path, for display and for recovery elsewhere.
    pub path: String,
    /// Hex-encoded address.
    pub address: String,
}

/// Where a broadcast transfer has got to.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransferStatus {
    /// Signed but not yet sent. An offline device stops here.
    Signed,
    /// Accepted into a node's mempool.
    Broadcast,
    /// Seen in a block.
    Confirmed,
    /// Rejected by the node.
    Failed,
}

/// A transfer the wallet has signed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingTransfer {
    /// The signed transaction.
    pub transfer: SignedTransfer,
    /// Current status.
    pub status: TransferStatus,
    /// Why it failed, when it did.
    pub detail: Option<String>,
}

/// An unlocked wallet session.
///
/// Holds the seed in [`Zeroizing`], so it is wiped when the session ends
/// rather than lingering in a freed allocation.
pub struct Wallet {
    seed: Zeroizing<[u8; SEED_LEN]>,
    accounts: BTreeMap<u32, Account>,
    history: Vec<PendingTransfer>,
}

impl core::fmt::Debug for Wallet {
    /// Redacted: a wallet in a log or panic message is a leaked seed.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Wallet")
            .field("seed", &"<redacted>")
            .field("accounts", &self.accounts.len())
            .field("history", &self.history.len())
            .finish()
    }
}

impl Wallet {
    /// Opens a session over an unlocked seed.
    #[must_use]
    pub fn from_seed(seed: Zeroizing<[u8; SEED_LEN]>) -> Self {
        Self {
            seed,
            accounts: BTreeMap::new(),
            history: Vec::new(),
        }
    }

    /// Opens a session from a recovery phrase.
    ///
    /// # Errors
    ///
    /// Propagates mnemonic validation failures.
    pub fn from_mnemonic(phrase: &str, passphrase: &str) -> Result<Self> {
        Ok(Self::from_seed(hd::seed_from_mnemonic(phrase, passphrase)?))
    }

    /// Derives and remembers account `index`.
    ///
    /// # Errors
    ///
    /// Propagates derivation failures.
    pub fn account(&mut self, index: u32) -> Result<Account> {
        if let Some(existing) = self.accounts.get(&index) {
            return Ok(existing.clone());
        }

        let path = DerivationPath::account(0, index);
        let address = hd::address_at(self.seed.as_ref(), &path)?;
        let account = Account {
            index,
            path: path.to_string(),
            address: hex::encode(address),
        };

        self.accounts.insert(index, account.clone());
        Ok(account)
    }

    /// Every account derived so far, in index order.
    #[must_use]
    pub fn accounts(&self) -> Vec<Account> {
        self.accounts.values().cloned().collect()
    }

    /// Signs a transfer from account `index`. No network required.
    ///
    /// # Errors
    ///
    /// Propagates derivation and signing failures.
    ///
    /// `terms` are what the node said while online: see [`SignTerms`].
    pub fn sign(
        &mut self,
        index: u32,
        recipient: &str,
        amount: u64,
        terms: &SignTerms<'_>,
    ) -> Result<SignedTransfer> {
        let path = DerivationPath::account(0, index);
        let signing_key = hd::signing_key_at(self.seed.as_ref(), &path)?;
        let SignTerms {
            fee,
            fee_to,
            nonce,
            chain,
        } = *terms;
        let transfer = match fee_to {
            Some(collector) => {
                check_fee_collector(collector)?;
                sign_transfer_to(
                    &signing_key,
                    recipient,
                    amount,
                    fee,
                    collector,
                    nonce,
                    chain,
                )?
            }
            None => sign_transfer(&signing_key, recipient, amount, fee, nonce, chain)?,
        };

        self.history.push(PendingTransfer {
            transfer: transfer.clone(),
            status: TransferStatus::Signed,
            detail: None,
        });

        Ok(transfer)
    }

    /// Bytes a fee-paying transfer from account `index` will occupy.
    ///
    /// # Errors
    ///
    /// Propagates derivation, address and signing failures.
    pub fn transfer_size(
        &self,
        index: u32,
        fee_to: &str,
        nonce: u64,
        chain: &ChainTag,
    ) -> Result<u64> {
        let path = DerivationPath::account(0, index);
        let signing_key = hd::signing_key_at(self.seed.as_ref(), &path)?;
        transfer_size(&signing_key, fee_to, nonce, chain)
    }

    /// Signed transfers, newest first.
    #[must_use]
    pub fn history(&self) -> Vec<PendingTransfer> {
        self.history.iter().rev().cloned().collect()
    }

    /// Records the outcome of a broadcast.
    pub fn record_outcome(&mut self, txid: &str, status: TransferStatus, detail: Option<String>) {
        if let Some(entry) = self
            .history
            .iter_mut()
            .find(|entry| entry.transfer.txid == txid)
        {
            entry.status = status;
            entry.detail = detail;
        }
    }
}

/// Broadcasts raw transaction hex to a node.
///
/// Takes hex rather than a [`Wallet`], so a transaction signed on an offline
/// device can be carried across by any means and broadcast from a machine that
/// never held the key.
///
/// # Errors
///
/// Returns [`WalletError::Node`] if the node is unreachable or rejects it.
pub async fn broadcast(node_url: &str, raw_hex: &str) -> Result<String> {
    let client = HttpClientBuilder::default()
        .build(node_url)
        .map_err(|e| WalletError::Node(format!("connecting to {node_url}: {e}")))?;

    let result: serde_json::Value = client
        .request("send_raw_transaction", rpc_params![raw_hex])
        .await
        .map_err(|e| WalletError::Node(e.to_string()))?;

    result
        .get("txid")
        .and_then(|txid| txid.as_str())
        .map(str::to_string)
        .ok_or_else(|| WalletError::Node(format!("unexpected response: {result}")))
}

/// What a node charges, and where the fee goes (ADR-029).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeeQuote {
    /// Whether this chain charges fees at all.
    pub active: bool,
    /// Base fee per serialized byte for the next block.
    pub base_fee: u64,
    /// Hex address the fee output must pay.
    pub collector: String,
}

/// Reads the node's current fee terms.
///
/// # Errors
///
/// Returns [`WalletError::Node`] if the node is unreachable.
pub async fn fee_quote(node_url: &str) -> Result<FeeQuote> {
    let client = HttpClientBuilder::default()
        .build(node_url)
        .map_err(|e| WalletError::Node(format!("connecting to {node_url}: {e}")))?;

    let info: custom_l1_node::rpc::FeeInfo = client
        .request("get_fee_info", rpc_params![])
        .await
        .map_err(|e| WalletError::Node(e.to_string()))?;

    if info.active {
        check_fee_collector(&info.collector)?;
    }
    Ok(FeeQuote {
        active: info.active,
        base_fee: info.base_fee,
        collector: info.collector,
    })
}

/// Everything a signature needs from the node, fetched while online and then
/// used offline: the fee and where it goes, the nonce, and the chain.
#[derive(Clone, Copy, Debug)]
pub struct SignTerms<'a> {
    /// The fee, in base units.
    pub fee: u64,
    /// The fee collector from [`fee_quote`] on a fee-market chain; `None`
    /// burns the fee, which only a chain without fees accepts.
    pub fee_to: Option<&'a str>,
    /// The account's next nonce.
    pub nonce: u64,
    /// The genesis the signature commits to (ADR-036), from [`chain_info`].
    pub chain: &'a ChainTag,
}

/// Fetches the chain's genesis block id (for transaction signing).
///
/// # Errors
///
/// Returns [`WalletError::Node`] if the node is unreachable.
pub async fn chain_info(node_url: &str) -> Result<ChainTag> {
    let client = HttpClientBuilder::default()
        .build(node_url)
        .map_err(|e| WalletError::Node(format!("connecting to {node_url}: {e}")))?;

    let info: custom_l1_node::rpc::ChainInfo = client
        .request("get_chain_info", rpc_params![])
        .await
        .map_err(|e| WalletError::Node(e.to_string()))?;

    info.chain_tag()
        .map_err(|e| WalletError::Node(e.to_string()))
}

/// What the home screen shows about the network: which one, and that it is
/// moving.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkStatus {
    /// The network's name (`maya-testnet-1`), if the node gave one.
    pub network: Option<String>,
    /// The genesis id, hex: the one chain this wallet's signatures are for.
    pub genesis: String,
    /// The node's tip height.
    pub height: u64,
}

/// Reads the network's name, genesis and tip height in one call.
///
/// # Errors
///
/// Returns [`WalletError::Node`] if the node is unreachable.
pub async fn network_status(node_url: &str) -> Result<NetworkStatus> {
    let client = HttpClientBuilder::default()
        .build(node_url)
        .map_err(|e| WalletError::Node(format!("connecting to {node_url}: {e}")))?;
    let info: custom_l1_node::rpc::ChainInfo = client
        .request("get_chain_info", rpc_params![])
        .await
        .map_err(|e| WalletError::Node(e.to_string()))?;
    Ok(NetworkStatus {
        network: info.chain_id,
        genesis: info.genesis,
        height: info.height,
    })
}

/// Reads an account's balance and next nonce from a node.
///
/// # Errors
///
/// Returns [`WalletError::Node`] if the node is unreachable.
pub async fn fetch_account(node_url: &str, address: &str) -> Result<(u64, u64)> {
    let client = HttpClientBuilder::default()
        .build(node_url)
        .map_err(|e| WalletError::Node(format!("connecting to {node_url}: {e}")))?;

    let info: custom_l1_node::rpc::AccountInfo = client
        .request("get_balance", rpc_params![address])
        .await
        .map_err(|e| WalletError::Node(e.to_string()))?;

    Ok((info.balance, info.nonce))
}
