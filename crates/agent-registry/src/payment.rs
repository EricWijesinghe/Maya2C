//! Quotes, payments and receipts for agent-to-agent payments (ADR-130).
//!
//! The chain sees only a transfer. The payee signs a quote; the payer sends an
//! ordinary transfer whose memo is the quote hash; the payee checks that
//! transfer and signs a receipt that names its txid. Anyone holding the
//! receipt and the transfer can check the binding.

use std::collections::BTreeSet;

use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, SignatureSuite};
use primitive_types::U256;
use serde::{Deserialize, Serialize};

use crate::card::{AgentId, MAX_TEXT_BYTES, agent_id_of};
use crate::error::{Error, Result};
use crate::signer::AgentSigner;
use crate::wire;

pub const QUOTE_DOMAIN: &[u8] = b"maya2c agent quote v1";
pub const RECEIPT_DOMAIN: &[u8] = b"maya2c agent receipt v1";

/// A payee's offer. `quote_id` is a nonce the payee picks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    #[serde(with = "wire::hex32")]
    pub quote_id: [u8; 32],
    #[serde(with = "wire::hex32")]
    pub payee: AgentId,
    #[serde(with = "wire::hex32")]
    pub payer: AgentId,
    pub capability: String,
    #[serde(with = "wire::dec_u256")]
    pub amount: U256,
    /// The account the payee receives into.
    #[serde(with = "wire::hex32")]
    pub payee_address: [u8; 32],
    /// Unix seconds. The quote is refused at and after this instant.
    pub expires_at: u64,
}

impl Quote {
    pub fn canonical(&self) -> Result<Vec<u8>> {
        let capability_ok = (1..=MAX_TEXT_BYTES).contains(&self.capability.len());
        if !capability_ok || self.amount.is_zero() {
            return Err(Error::Bounds);
        }
        let mut out = Vec::new();
        wire::put(&mut out, QUOTE_DOMAIN);
        out.extend_from_slice(&self.quote_id);
        out.extend_from_slice(&self.payee);
        out.extend_from_slice(&self.payer);
        wire::put(&mut out, self.capability.as_bytes());
        wire::put_u256(&mut out, self.amount);
        out.extend_from_slice(&self.payee_address);
        out.extend_from_slice(&self.expires_at.to_be_bytes());
        Ok(out)
    }

    /// The memo a payment carries: BLAKE3 of the canonical quote.
    pub fn hash(&self) -> Result<[u8; 32]> {
        Ok(*blake3::hash(&self.canonical()?).as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedQuote {
    pub quote: Quote,
    #[serde(with = "wire::hex_bytes")]
    pub signature: Vec<u8>,
}

impl SignedQuote {
    pub fn sign(quote: Quote, signer: &AgentSigner) -> Result<Self> {
        if quote.payee != signer.agent_id() {
            return Err(Error::NotPayee);
        }
        let signature = signer.sign(&quote.canonical()?)?;
        Ok(Self { quote, signature })
    }

    /// Checks the payee's signature, then the expiry. Returns the quote hash.
    pub fn verify(&self, payee_public_key: &[u8], now: u64) -> Result<[u8; 32]> {
        if agent_id_of(payee_public_key) != self.quote.payee {
            return Err(Error::Signature);
        }
        let canonical = self.quote.canonical()?;
        HybridMlDsa65SlhDsa128s::verify(payee_public_key, &canonical, &self.signature)
            .map_err(|_| Error::Signature)?;
        if now >= self.quote.expires_at {
            return Err(Error::Expired);
        }
        self.quote.hash()
    }
}

/// What the payer's wallet signs: an ordinary transfer. The node applies the
/// session limits (ADR-107); this crate does not sign or submit anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentDraft {
    #[serde(with = "wire::hex32")]
    pub to: [u8; 32],
    #[serde(with = "wire::dec_u256")]
    pub amount: U256,
    #[serde(with = "wire::hex32")]
    pub memo: [u8; 32],
}

/// The payer's check before paying: the quote is theirs, unexpired and signed.
pub fn draft_payment(
    quote: &SignedQuote,
    payee_public_key: &[u8],
    payer: &AgentId,
    now: u64,
) -> Result<PaymentDraft> {
    if quote.quote.payer != *payer {
        return Err(Error::NotPayer);
    }
    let memo = quote.verify(payee_public_key, now)?;
    Ok(PaymentDraft {
        to: quote.quote.payee_address,
        amount: quote.quote.amount,
        memo,
    })
}

/// A transfer as the chain reports it, reduced to the fields a receipt binds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    #[serde(with = "wire::hex32")]
    pub txid: [u8; 32],
    #[serde(with = "wire::hex32")]
    pub to: [u8; 32],
    #[serde(with = "wire::dec_u256")]
    pub amount: U256,
    #[serde(with = "wire::hex32")]
    pub memo: [u8; 32],
    pub height: u64,
}

/// Quotes already paid. Held in memory, so a restart forgets them.
/// shortcut: in-memory replay set; persist it before more than one process
/// settles quotes.
#[derive(Debug, Default)]
pub struct PaymentBook {
    settled: BTreeSet<[u8; 32]>,
}

impl PaymentBook {
    pub fn settle(&mut self, quote_hash: [u8; 32]) -> Result<()> {
        if self.settled.insert(quote_hash) {
            Ok(())
        } else {
            Err(Error::Replay)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    #[serde(with = "wire::hex32")]
    pub quote_hash: [u8; 32],
    #[serde(with = "wire::hex32")]
    pub txid: [u8; 32],
    #[serde(with = "wire::hex32")]
    pub payer: AgentId,
    #[serde(with = "wire::hex32")]
    pub payee: AgentId,
    #[serde(with = "wire::dec_u256")]
    pub amount: U256,
    pub height: u64,
}

impl Receipt {
    pub fn canonical(&self) -> Vec<u8> {
        let mut out = Vec::new();
        wire::put(&mut out, RECEIPT_DOMAIN);
        out.extend_from_slice(&self.quote_hash);
        out.extend_from_slice(&self.txid);
        out.extend_from_slice(&self.payer);
        out.extend_from_slice(&self.payee);
        wire::put_u256(&mut out, self.amount);
        out.extend_from_slice(&self.height.to_be_bytes());
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedReceipt {
    pub receipt: Receipt,
    #[serde(with = "wire::hex_bytes")]
    pub signature: Vec<u8>,
}

/// Payee side. Refuses unless the transfer pays this quote's memo, address and
/// amount; then marks the quote spent and signs the receipt.
pub fn issue_receipt(
    book: &mut PaymentBook,
    signer: &AgentSigner,
    quote: &SignedQuote,
    transfer: &Transfer,
    now: u64,
) -> Result<SignedReceipt> {
    if quote.quote.payee != signer.agent_id() {
        return Err(Error::NotPayee);
    }
    let quote_hash = quote.verify(signer.public_key(), now)?;
    let paid = transfer.memo == quote_hash
        && transfer.to == quote.quote.payee_address
        && transfer.amount == quote.quote.amount;
    if !paid {
        return Err(Error::Mismatch);
    }
    book.settle(quote_hash)?;
    let receipt = Receipt {
        quote_hash,
        txid: transfer.txid,
        payer: quote.quote.payer,
        payee: quote.quote.payee,
        amount: transfer.amount,
        height: transfer.height,
    };
    let signature = signer.sign(&receipt.canonical())?;
    Ok(SignedReceipt { receipt, signature })
}

/// Verifier side: the receipt is the payee's and it describes this transfer.
pub fn verify_receipt(
    signed: &SignedReceipt,
    payee_public_key: &[u8],
    transfer: &Transfer,
) -> Result<()> {
    let receipt = &signed.receipt;
    if agent_id_of(payee_public_key) != receipt.payee {
        return Err(Error::Signature);
    }
    HybridMlDsa65SlhDsa128s::verify(payee_public_key, &receipt.canonical(), &signed.signature)
        .map_err(|_| Error::Signature)?;
    let bound = transfer.txid == receipt.txid
        && transfer.memo == receipt.quote_hash
        && transfer.amount == receipt.amount
        && transfer.height == receipt.height;
    if bound { Ok(()) } else { Err(Error::Mismatch) }
}
