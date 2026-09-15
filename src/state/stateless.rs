//! Stateless verification of transfer blocks. Research branch.
//!
//! ## The switch
//!
//! From [`STATELESS_ACTIVATION_HEIGHT`] the block being staged writes one
//! record, `sl:accounts-sparse`, and from then on the accounts root under the
//! state root is `maya-stateless-core`'s sparse tree instead of
//! `state::merkle`'s address-ordered one. The marker sits under its own layer,
//! so the switch is itself committed, journalled and reverted like any record,
//! and `state_root()` can tell which tree to build without being told a
//! height. The activation block's *pre*-state is still dense, so the first
//! block a stateless node can check is the one after it.
//!
//! ## What a witness can decide
//!
//! A block whose transactions are all `TxKind::Transfer`, on a state with no
//! `m:`, `o:` or `g:` records. That is not caution but the list of everything
//! `StateDB::stage_block` does besides staging transactions:
//!
//! | Pass | Reads | Inert when |
//! |---|---|---|
//! | `settle_sealed` | envelopes due at this height (`m:`) | the sealed layer is absent |
//! | `settle_trading` | pairs this block's DEX transactions touched | no transaction is a DEX kind |
//! | `settle_oracle` | the authority registry (`o:`) | the oracle layer is absent |
//! | `settle_governance` | every proposal (`g:`) | the governance layer is absent |
//! | `check_anomalies` | pool records and the shielded pool the block wrote | no transaction is a DEX or shielded kind |
//!
//! Conservation holds by construction for transfers — a debit of exactly the
//! output total — and a transfer maps to no breaker module. Anything else is
//! [`StatelessError::Unverifiable`]: a stateless node cannot say, which is not
//! the same as saying no.
//!
//! ## Invalid is a verdict; unverifiable is not
//!
//! A witness is not under the block id or the proof of work. A relay can
//! corrupt it, and a body that fails its header's `tx_root` may be a relay's
//! substitution under an honest header — invariant 24's censorship case. So
//! both are `Unverifiable`, and the caller fetches again. `Invalid` is reserved
//! for what the block's own contents decide once the witness has matched the
//! parent root: a bad signature, a broken transfer rule, or a declared state
//! root the transfers do not produce.

use std::collections::BTreeMap;

use maya_stateless_core::{Blake3, PartialTree, apply_transfer, sparse};

use crate::core::codec::ByteReader;
use crate::core::payload::TxKind;
use crate::core::{Block, Transaction};
use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::merkle::HASH_LEN;
use crate::state::proof::{LAYER_ORDER, LayerDigest, StateLayer};

pub use crate::state::context::STATELESS_ACTIVATION_HEIGHT;

/// Prefix of the stateless layer's records.
pub const STATELESS_PREFIX: &[u8] = b"sl:";

/// The one record under it: present means accounts are a sparse tree.
pub const SPARSE_ACCOUNTS_KEY: &[u8] = b"sl:accounts-sparse";

/// Layers whose records an end-of-block pass may act on without a transaction
/// naming them. See the module table.
const PASS_LAYERS: [StateLayer; 3] = [
    StateLayer::Sealed,
    StateLayer::Oracle,
    StateLayer::Governance,
];

/// What a stateless node needs beside a block: the touched accounts, their
/// paths, and the layer digests folded above the accounts root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateWitness {
    /// The accounts tree, opened at every key the block reads.
    pub accounts: PartialTree<[u8; HASH_LEN]>,
    /// Every layer present in the pre-state, in fold order.
    pub layers: Vec<LayerDigest>,
}

impl StateWitness {
    /// Length-prefixed tree, then the layer count and each `(tag, root)`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut tree = Vec::new();
        self.accounts.encode::<Blake3>(&mut tree);
        let mut out = Vec::with_capacity(16 + tree.len() + self.layers.len() * 33);
        out.extend_from_slice(&(tree.len() as u64).to_le_bytes());
        out.extend_from_slice(&tree);
        out.extend_from_slice(&(self.layers.len() as u64).to_le_bytes());
        for digest in &self.layers {
            out.push(digest.layer.tag());
            out.extend_from_slice(&digest.root);
        }
        out
    }

    /// Decodes the one canonical encoding.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a malformed or non-canonical tree, an unknown
    /// layer, layers out of fold order or repeated, or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let tree_len = reader.read_collection_len(1)?;
        let accounts = PartialTree::decode(&Blake3, reader.read_slice(tree_len)?)
            .map_err(|defect| NodeError::Decode(defect.to_string()))?;

        let count = reader.read_collection_len(1 + HASH_LEN)?;
        let mut layers = Vec::with_capacity(count);
        let mut floor = 0;
        for _ in 0..count {
            let tag = reader.read_u8()?;
            let layer = StateLayer::from_tag(tag)
                .ok_or_else(|| NodeError::Decode(format!("unknown state layer {tag}")))?;
            // Order is consensus, so a second ordering is a second encoding.
            let position = LAYER_ORDER
                .iter()
                .position(|candidate| *candidate == layer)
                .unwrap_or(usize::MAX);
            if position < floor {
                return Err(NodeError::Decode(format!(
                    "layer {} is out of order or repeated",
                    layer.label()
                )));
            }
            floor = position + 1;
            layers.push(LayerDigest {
                layer,
                root: reader.read_array::<HASH_LEN>()?,
            });
        }
        reader.finish()?;
        Ok(Self { accounts, layers })
    }
}

/// Why a stateless node would not accept a block.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum StatelessError {
    /// The block breaks a rule. Every full node refuses it too.
    #[error("block is invalid: {0}")]
    Invalid(String),
    /// This witness and body cannot decide the block. Fetch them again.
    #[error("block is not verifiable from this witness: {0}")]
    Unverifiable(String),
}

impl From<maya_stateless_core::Error> for StatelessError {
    fn from(error: maya_stateless_core::Error) -> Self {
        match error {
            maya_stateless_core::Error::Invalid(violation) => Self::Invalid(violation.to_string()),
            maya_stateless_core::Error::Unverifiable(defect) => {
                Self::Unverifiable(defect.to_string())
            }
        }
    }
}

impl StateDB {
    /// Writes the sparse-accounts marker in the first block at or past
    /// activation. Called from `stage_block`, so every commit path agrees.
    pub(crate) fn mark_sparse_accounts(
        &self,
        overlay: &mut Overlay,
        context: BlockContext,
    ) -> Result<()> {
        if context.stateless_active() && self.record(overlay, SPARSE_ACCOUNTS_KEY)?.is_none() {
            Self::put_record(overlay, SPARSE_ACCOUNTS_KEY.to_vec(), vec![1]);
        }
        Ok(())
    }

    /// Whether the accounts root of this state, with `overlay`, is sparse.
    pub(crate) fn sparse_accounts(&self, overlay: &Overlay) -> Result<bool> {
        Ok(self.record(overlay, SPARSE_ACCOUNTS_KEY)?.is_some())
    }

    /// A witness for `block` against committed state: its pre-state.
    ///
    /// Built for any block; whether the block is one a witness can decide is
    /// [`verify_block`]'s question.
    ///
    /// # Errors
    ///
    /// [`NodeError::MalformedProof`] before activation, when no sparse root
    /// exists to open; [`NodeError::Storage`] on a read failure.
    pub fn block_witness(&self, block: &Block) -> Result<StateWitness> {
        let touched: Vec<Address> = block.transactions.iter().flat_map(touched_by).collect();
        self.witness_for(&touched)
    }

    /// A witness for one transaction against the current tip, for a wallet to
    /// broadcast beside it. Stale after the next block.
    ///
    /// # Errors
    ///
    /// As [`StateDB::block_witness`].
    pub fn transaction_witness(&self, tx: &Transaction) -> Result<StateWitness> {
        let touched: Vec<Address> = touched_by(tx).collect();
        self.witness_for(&touched)
    }

    fn witness_for(&self, touched: &[Address]) -> Result<StateWitness> {
        if !self.sparse_accounts(&Overlay::default())? {
            return Err(NodeError::MalformedProof {
                reason: "accounts are not a sparse tree before stateless activation".to_string(),
            });
        }
        let leaves: Vec<_> = self
            .all_accounts()?
            .iter()
            .map(|(address, account)| (*address, account.encode()))
            .collect();
        let accounts = sparse::open(&Blake3, &leaves, touched).map_err(|defect| {
            NodeError::MalformedProof {
                reason: defect.to_string(),
            }
        })?;
        Ok(StateWitness {
            accounts,
            layers: self.state_layers(&Overlay::default())?,
        })
    }
}

/// The sparse accounts root of `accounts`.
///
/// # Errors
///
/// None in practice — a `BTreeMap` is sorted — but reported rather than
/// unwrapped, as `account_proof` reports its unreachable case.
pub(crate) fn sparse_accounts_root(
    accounts: &BTreeMap<Address, Account>,
) -> Result<[u8; HASH_LEN]> {
    let leaves: Vec<_> = accounts
        .iter()
        .map(|(address, account)| (*address, account.encode()))
        .collect();
    sparse::root(&Blake3, &leaves).map_err(|defect| NodeError::MalformedProof {
        reason: defect.to_string(),
    })
}

/// Every account a transfer reads: its sender, then its recipients.
fn touched_by(tx: &Transaction) -> impl Iterator<Item = Address> + '_ {
    core::iter::once(tx.sender()).chain(tx.outputs.iter().map(|output| output.recipient))
}

fn fold_layers(accounts_root: &[u8; HASH_LEN], layers: &[LayerDigest]) -> [u8; HASH_LEN] {
    layers.iter().fold(*accounts_root, |root, digest| {
        digest.layer.fold(&root, &digest.root)
    })
}

/// Verifies `block` from `witness` against its parent's state root, reading no
/// database.
///
/// # Errors
///
/// [`StatelessError::Invalid`] for a bad signature, a broken transfer rule, or
/// a declared state root the transfers do not produce.
/// [`StatelessError::Unverifiable`] for a body that fails `tx_root`, a block a
/// witness cannot decide, or a witness that does not match the parent root or
/// does not open a key the block reads.
pub fn verify_block(
    parent_state_root: &[u8; HASH_LEN],
    block: &Block,
    witness: StateWitness,
) -> core::result::Result<(), StatelessError> {
    block
        .check_tx_root()
        .map_err(|error| StatelessError::Unverifiable(error.to_string()))?;
    check_eligible(block, &witness.layers)?;

    let StateWitness { accounts, layers } = witness;
    let mut tree = accounts
        .verify(&Blake3, |root| {
            fold_layers(root, &layers) == *parent_state_root
        })
        .map_err(|defect| StatelessError::Unverifiable(defect.to_string()))?;

    for tx in &block.transactions {
        tx.verify()
            .map_err(|error| StatelessError::Invalid(error.to_string()))?;
        let outputs = tx
            .outputs
            .iter()
            .map(|output| (output.recipient, output.amount));
        apply_transfer(&mut tree, tx.sender(), tx.nonce, outputs)?;
    }

    let executed = fold_layers(&tree.digest(&Blake3), &layers);
    if executed != block.header.state_root {
        return Err(StatelessError::Invalid(format!(
            "header declares state root {}, its transfers produce {}",
            hex::encode(block.header.state_root),
            hex::encode(executed)
        )));
    }
    Ok(())
}

fn check_eligible(
    block: &Block,
    layers: &[LayerDigest],
) -> core::result::Result<(), StatelessError> {
    if let Some(tx) = block
        .transactions
        .iter()
        .find(|tx| !matches!(tx.kind, TxKind::Transfer))
    {
        return Err(StatelessError::Unverifiable(format!(
            "a {} transaction reads state no witness names",
            tx.kind.label()
        )));
    }
    let present = |layer: StateLayer| layers.iter().any(|digest| digest.layer == layer);
    if !present(StateLayer::Stateless) {
        return Err(StatelessError::Unverifiable(
            "accounts are not a sparse tree in the parent state".to_string(),
        ));
    }
    if let Some(layer) = PASS_LAYERS.into_iter().find(|layer| present(*layer)) {
        return Err(StatelessError::Unverifiable(format!(
            "the {} layer holds records an end-of-block pass may act on",
            layer.label()
        )));
    }
    Ok(())
}
