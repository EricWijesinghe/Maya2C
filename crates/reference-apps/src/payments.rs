//! Point-of-sale payments.
//!
//! The customer authorises a till once: a session key that may pay **only this
//! merchant**, **at most this much per payment**, **until this height**. The
//! till then charges without a wallet prompt per purchase, and a stolen till
//! key can do nothing outside that scope. The customer's daily limit applies
//! on top, whatever key signs.
//!
//! Each payment produces a receipt line rendered by the clear-signing crate,
//! the same text the wallet showed when the session was authorised.

use maya_clear_sign::{Effect, render};
use maya_crypto_pq::suite::SuiteId;
use maya_smart_account::account::AccountError;
use maya_smart_account::op::Action;
use maya_smart_account::policy::{Policy, SessionScope};
use maya_smart_account::{AccountId, Registry};

use crate::wallet::{Key, Wallet};

/// What the customer agrees to at the till.
#[derive(Clone, Debug)]
pub struct Checkout {
    /// The merchant, the only recipient the till may pay.
    pub merchant: AccountId,
    /// Largest single payment.
    pub max_per_payment: u64,
    /// Height after which the till key is dead.
    pub expires_at: u64,
}

/// The one-time authorisation: a daily cap on the account and a scoped till key.
///
/// # Errors
///
/// [`AccountError`] if the registry refuses either op.
pub fn authorise_till(
    registry: &mut Registry,
    customer: &mut Wallet,
    till: &Key,
    checkout: &Checkout,
    daily_limit: u64,
    height: u64,
) -> Result<(), AccountError> {
    let policy = customer.signed(Action::SetPolicy(Policy {
        daily_limit,
        ..Policy::default()
    }));
    registry.apply(&policy, height, 0)?;
    let session = customer.signed(Action::AddSessionKey {
        suite: SuiteId::MlDsa65,
        public_key: till.pk.clone(),
        scope: SessionScope {
            recipients: vec![checkout.merchant],
            max_amount: checkout.max_per_payment,
            expires_at: checkout.expires_at,
        },
    });
    registry.apply(&session, height, 0)
}

/// The till charges `amount` to the customer. Returns the receipt line.
///
/// # Errors
///
/// [`AccountError::OutOfScope`] for a wrong recipient, an amount over the
/// per-payment cap or an expired session; [`AccountError::Policy`] past the
/// daily limit.
pub fn charge(
    registry: &mut Registry,
    customer: &mut Wallet,
    till: &Key,
    to: AccountId,
    amount: u64,
    height: u64,
) -> Result<String, AccountError> {
    let op = till.sign(customer.op(Action::Transfer { to, amount }));
    if let Err(e) = registry.apply(&op, height, 0) {
        customer.refused();
        return Err(e);
    }
    Ok(render(&Effect::Transfer {
        token: [0; 32],
        to,
        amount: u128::from(amount),
    }))
}
