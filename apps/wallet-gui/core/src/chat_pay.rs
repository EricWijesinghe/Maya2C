//! The wallet's side of chat payments (ADR-088, amendment 1).
//!
//! - **Bindings.** A chat identity holds no spending key. The wallet signs,
//!   with an account's own key, "chat identity X may name this account for
//!   payment requests on chain C until T" ([`sign_binding`]), or a
//!   revocation of every earlier binding of X. The chat carries the result;
//!   `maya-chat`'s `pay::binding` checks it. The wallet builds the signed
//!   bytes itself from what it shows its user and never signs bytes a chat
//!   hands it.
//! - **Pay links.** A `maya2c-wallet://pay` link from the chat names its
//!   request id and chain. [`check_pay_chain`] refuses a link for another
//!   chain, and [`PaidRequests`] refuses a request id this wallet has paid.

use std::collections::BTreeMap;
use std::path::Path;

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use serde::{Deserialize, Serialize};

use crate::error::{Result, WalletError};

/// Longest a binding may run, seconds. Must equal `maya-chat`'s
/// `pay::binding::MAX_BINDING_TTL`, which refuses anything longer.
pub const MAX_BINDING_TTL: u64 = 90 * 24 * 3_600;
/// What the binding command offers when the user names no lifetime. A
/// policy choice, not a measurement.
pub const DEFAULT_BINDING_TTL: u64 = 30 * 24 * 3_600;
/// Paid request ids kept, at most. A request lives a day at most in the
/// chat (`MAX_TTL`), so this is far more than a wallet pays in that time.
pub const MAX_PAID: usize = 4_096;
/// How long a paid request id is kept, seconds: past any request's life.
pub const PAID_KEEP: u64 = 7 * 24 * 3_600;

/// What `maya-chat`'s `pay::binding::message` puts first.
const MESSAGE_DOMAIN: &[u8] = b"maya2c wallet chat binding v1\0";

/// What a binding does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindingKind {
    /// The chat identity may name the account.
    Bind,
    /// Every earlier binding of the chat identity is void.
    Revoke,
}

/// The fields a binding signs, other than the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingTerms {
    /// Bind or revoke.
    pub kind: BindingKind,
    /// The chain's genesis id, as the node reports it.
    pub genesis: [u8; 32],
    /// The chat identity's address.
    pub chat_id: [u8; 32],
    /// Unix seconds; also the binding's sequence, so a later binding must
    /// carry a later time.
    pub issued_at: u64,
    /// Unix seconds after which the binding is void.
    pub expires_at: u64,
}

/// The bytes a binding's signature covers, for `account`.
#[must_use]
pub fn binding_message(terms: &BindingTerms, account: &[u8; 32]) -> Vec<u8> {
    let kind = match terms.kind {
        BindingKind::Bind => 1,
        BindingKind::Revoke => 2,
    };
    let mut out = MESSAGE_DOMAIN.to_vec();
    out.push(kind);
    out.extend_from_slice(&terms.genesis);
    out.extend_from_slice(&terms.chat_id);
    out.extend_from_slice(account);
    out.extend_from_slice(&terms.issued_at.to_le_bytes());
    out.extend_from_slice(&terms.expires_at.to_le_bytes());
    out
}

/// Signs a binding with `key`, the account's hybrid key, and returns it in
/// the chat's wire form: the fields, the public key, the signature.
///
/// # Errors
///
/// [`WalletError::Signing`] for a lifetime the chat would refuse, or a
/// signing failure.
pub fn sign_binding(key: &HybridSigningKey, terms: &BindingTerms) -> Result<Vec<u8>> {
    let life = terms.expires_at.checked_sub(terms.issued_at).unwrap_or(0);
    if life == 0 || life > MAX_BINDING_TTL {
        return Err(WalletError::Signing(format!(
            "a chat binding lasts between 1 second and {MAX_BINDING_TTL} seconds"
        )));
    }
    let message = binding_message(terms, &key.address());
    let signature = key
        .sign(&message)
        .map_err(|e| WalletError::Signing(e.to_string()))?;
    let mut out = message[MESSAGE_DOMAIN.len()..].to_vec();
    key.public_key().encode_into(&mut out);
    signature.encode_into(&mut out);
    Ok(out)
}

/// Refuses a pay link whose `chain` is not `genesis`, the chain the wallet
/// is about to sign for.
///
/// # Errors
///
/// [`WalletError::Signing`] for another chain.
pub fn check_pay_chain(link_chain: &[u8; 32], genesis: &[u8; 32]) -> Result<()> {
    if link_chain == genesis {
        Ok(())
    } else {
        Err(WalletError::Signing(
            "the payment request is for another chain; nothing was signed".to_string(),
        ))
    }
}

/// Request ids this wallet has signed a payment for, with when. Kept on
/// disk, so a request link cannot be paid twice across restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaidRequests {
    paid: BTreeMap<String, u64>,
}

impl PaidRequests {
    /// Refuses `id` if it has been paid.
    ///
    /// # Errors
    ///
    /// [`WalletError::Signing`] for a paid request.
    pub fn check(&self, id: &[u8; 32]) -> Result<()> {
        if self.paid.contains_key(&hex::encode(id)) {
            return Err(WalletError::Signing(
                "this payment request was already paid; nothing was signed".to_string(),
            ));
        }
        Ok(())
    }

    /// Records `id` as paid at `now`, dropping ids older than [`PAID_KEEP`].
    ///
    /// # Errors
    ///
    /// [`WalletError::Signing`] if already paid, or if [`MAX_PAID`] recent
    /// ids are held.
    pub fn record(&mut self, id: &[u8; 32], now: u64) -> Result<()> {
        self.check(id)?;
        self.paid
            .retain(|_, at| at.saturating_add(PAID_KEEP) > now);
        if self.paid.len() >= MAX_PAID {
            return Err(WalletError::Signing(
                "too many payment requests paid recently".to_string(),
            ));
        }
        self.paid.insert(hex::encode(id), now);
        Ok(())
    }

    /// Reads the list from `path`; a missing file is an empty list.
    ///
    /// # Errors
    ///
    /// [`WalletError::Format`] for an unreadable file.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| WalletError::Format(e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(WalletError::Format(e.to_string())),
        }
    }

    /// Writes the list to `path`, whole or not at all: a temporary file
    /// renamed over the old one.
    ///
    /// # Errors
    ///
    /// [`WalletError::Format`] if the write fails.
    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec(self).map_err(|e| WalletError::Format(e.to_string()))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| WalletError::Format(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| WalletError::Format(e.to_string()))
    }
}
