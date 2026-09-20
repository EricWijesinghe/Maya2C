//! The chain side of the ISO 20022 bridge: bank accounts to chain addresses,
//! and committed state back to a statement.
//!
//! `maya_iso20022` deliberately knows nothing about blocks. This module is the
//! other half of that seam — it is where a [`PaymentIntent`] becomes something
//! the chain can hold, and where committed state becomes a `camt.053` a
//! counterparty can reconcile against.
//!
//! ## Virtual accounts
//!
//! A bank account has no Maya2C keypair. So each one is mapped to an address
//! **derived** from its list identifier rather than generated: the same IBAN
//! always maps to the same address, on every node, without anybody storing a
//! table. That matters twice over — a table is state that can disagree between
//! nodes, and a table is a thing an operator can edit to redirect a payment.
//!
//! Nobody holds the private key to a derived address, which is the point:
//! value at one moves only through the bridge, because there is no signature
//! that could move it any other way. It also means value there is
//! **unrecoverable** if the bridge is switched off, which is stated here rather
//! than discovered.
//!
//! ## What a statement is drawn from
//!
//! [`StatementSource::for_block`] reads a committed block only. It never reads
//! an overlay, a mempool, or a block that has not applied — a
//! statement is what a bank reconciles against, and reporting a payment that a
//! reorg then removes is worse than reporting it late. Every entry it renders
//! is `BOOK`, because there is no other status a committed block can produce.

use maya_iso20022::amount::Amount;
use maya_iso20022::bridge::PaymentIntent;
use maya_iso20022::camt053::{Direction, Entry, Statement};
use maya_iso20022::party::{AccountId, Currency};
use maya_iso20022::sanctions::{Identifier, identifier_for_account};

use crate::core::{Transaction, TxOutput};
use crate::error::{NodeError, Result};
use crate::state::{Address, StateDB};

/// Domain separating a bridge address from every other address derivation.
///
/// Without it, a derived address could collide with one somebody holds a key
/// to — and a collision here is a bank account whose balance a stranger can
/// spend.
const DOMAIN_VIRTUAL_ACCOUNT: &[u8] = b"maya-iso20022-virtual-account-v1:";

/// The chain address a bank account maps to.
///
/// Derived, never stored. See the module docs for why.
#[must_use]
pub fn address_for(identifier: &Identifier) -> Address {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN_VIRTUAL_ACCOUNT);
    hasher.update(identifier);
    *hasher.finalize().as_bytes()
}

/// The chain address for an ISO 20022 account.
#[must_use]
pub fn address_for_account(account: &AccountId) -> Address {
    address_for(&identifier_for_account(account))
}

/// The unsigned transaction an intent instructs.
///
/// Unsigned deliberately: the gateway's key signs it, and this function does
/// not hold one. A function that both decided what a payment was *and* signed
/// it would be a function whose only check is that it was called.
///
/// # Errors
///
/// Returns [`NodeError::InsufficientBalance`] if the debtor's derived address
/// holds less than the amount — checked here so a payment that cannot settle is
/// refused at the gateway, where a bank can be told why, rather than failing a
/// whole block later.
pub fn transaction_for(db: &StateDB, intent: &PaymentIntent, nonce: u64) -> Result<Transaction> {
    let debtor = address_for_account(&intent.debtor.account);
    let creditor = address_for_account(&intent.creditor.account);
    let amount = intent.amount.base_units();

    let balance = db.get_account(&debtor)?.balance;
    if balance < amount {
        return Err(NodeError::InsufficientBalance {
            address: hex::encode(debtor),
            required: amount,
            available: balance,
        });
    }

    Ok(Transaction::new(
        Vec::new(),
        vec![TxOutput {
            recipient: creditor,
            amount,
        }],
        nonce,
    ))
}

/// Bytes of a transaction id an entry reference carries.
///
/// `NtryRef` is `Max35Text`, and no binary-to-text encoding puts 32 bytes in 35
/// characters — hex needs 64, base58 needs 44. So the reference is a **prefix**,
/// and 16 bytes is the longest that fits as hex with a character to spare.
///
/// Truncating is safe for what the field is for and unsafe for what it is not.
/// It is a reconciliation handle: a bank matching one statement line against
/// one block, where 128 bits makes a collision not worth reasoning about. It is
/// not an identifier to look a transaction up by without checking — the full id
/// is in the block, and anything resolving a dispute should read it from there
/// rather than trusting 16 bytes somebody sent.
pub const ENTRY_REFERENCE_BYTES: usize = 16;

/// One settled payment, as a statement line.
///
/// The transaction id is the entry reference, so a line on a statement points
/// at the block that produced it and a dispute has something to resolve
/// against.
#[derive(Clone, Debug)]
pub struct Settled {
    /// The transaction that moved the value.
    pub txid: [u8; 32],
    /// How much moved.
    pub amount: Amount,
    /// Which way, from the statement account's point of view.
    pub direction: Direction,
}

impl Settled {
    /// Every payment in `block` that touched `account`, in block order.
    ///
    /// Reads the block's transactions rather than the state, because a balance
    /// is a total and a statement is a list — two payments that net to zero are
    /// invisible in the balance and are two lines here.
    #[must_use]
    pub fn in_block(block: &crate::core::Block, account: &Address) -> Vec<Self> {
        let mut settled = Vec::new();
        for transaction in &block.transactions {
            let txid = transaction.txid();
            let sender = transaction.sender();
            for output in &transaction.outputs {
                if output.recipient == *account {
                    settled.push(Self {
                        txid,
                        amount: Amount::from_base_units(output.amount),
                        direction: Direction::Credit,
                    });
                }
                if sender == *account {
                    settled.push(Self {
                        txid,
                        amount: Amount::from_base_units(output.amount),
                        direction: Direction::Debit,
                    });
                }
            }
        }
        settled
    }
}

/// Where a statement's numbers come from.
pub struct StatementSource;

impl StatementSource {
    /// Builds a `camt.053` for one account over one block.
    ///
    /// The opening balance is supplied rather than read: it is the closing
    /// balance of the previous statement, and re-deriving it from state would
    /// give the balance *now* rather than the balance the period started at.
    /// The closing balance is then computed by
    /// [`Statement::seal`](maya_iso20022::camt053::Statement::seal) from the
    /// entries, which is what makes the statement internally checkable.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the entries do not produce a
    /// representable closing balance — an account that would go negative, or
    /// past `u64`.
    pub fn for_block(
        message_id: &str,
        statement_id: &str,
        created_at: &str,
        account: AccountId,
        currency: Currency,
        opening: Amount,
        block: &crate::core::Block,
    ) -> Result<Statement> {
        let address = address_for_account(&account);
        let entries = Settled::in_block(block, &address)
            .into_iter()
            .map(|settled| Entry {
                reference: Some(hex::encode(&settled.txid[..ENTRY_REFERENCE_BYTES])),
                amount: settled.amount,
                direction: settled.direction,
            })
            .collect();

        Statement::seal(
            message_id,
            statement_id,
            created_at,
            account,
            currency,
            opening,
            entries,
        )
        .map_err(|error| NodeError::Decode(format!("camt.053: {error}")))
    }
}
