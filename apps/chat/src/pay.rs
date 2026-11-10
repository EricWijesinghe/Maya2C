//! Payment requests and payment receipts (ADR-088).
//!
//! A payee asks for money with a [`Request`] signed by its chat identity. The
//! payer's wallet pays, and the payer's chat says so with a [`Paid`] signed by
//! its identity. Neither message moves money or holds a wallet key: the chain
//! is the authority, and the payee checks it with [`settles_on_chain`].

use std::collections::BTreeMap;

use maya_crypto_pq::suite::{self, SuiteId};
use serde::{Deserialize, Serialize};

use crate::identity::{Identity, address_of};
use crate::{Address, ChatError};

/// Marks a session plaintext as a payment request.
pub const MAGIC: &[u8] = b"\0maya-chat pay v1\0";
/// Marks a session plaintext as a payment receipt.
pub const PAID_MAGIC: &[u8] = b"\0maya-chat paid v1\0";
/// Longest memo, in UTF-8 bytes (ADR-088 decision 3).
pub const MAX_MEMO_BYTES: usize = 140;
/// Most digits in an amount: the decimal length of the 256-bit maximum.
pub const MAX_AMOUNT_DIGITS: usize = 78;
/// Longest a request may run from the moment it is checked, seconds. A policy
/// bound on how long a pay button stays live, not a measurement.
pub const MAX_TTL: u64 = 24 * 3_600;
/// Blocks the payee scans for a payment before giving up. A policy bound.
pub const MAX_SCAN_BLOCKS: u64 = 10_000;
/// Requests a ledger holds at once, in each direction.
pub const MAX_LEDGER: usize = 4_096;

const REQUEST_DOMAIN: &str = "maya-chat 2026-11 pay request v1";
const PAID_DOMAIN: &str = "maya-chat 2026-11 pay paid v1";
const NONCE_LEN: usize = 16;
const U256_MAX: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

fn refuse(why: &'static str) -> ChatError {
    ChatError::Refused(why)
}

/// Checks an amount: canonical decimal base units, nonzero, at most 256 bits.
///
/// # Errors
///
/// [`ChatError::Refused`] for any other text.
pub fn check_amount(amount: &str) -> Result<(), ChatError> {
    let bytes = amount.as_bytes();
    let canonical = !bytes.is_empty()
        && bytes.len() <= MAX_AMOUNT_DIGITS
        && bytes.iter().all(u8::is_ascii_digit)
        && bytes[0] != b'0';
    if !canonical {
        return Err(refuse("amount is not a canonical decimal"));
    }
    if bytes.len() == MAX_AMOUNT_DIGITS && amount > U256_MAX {
        return Err(refuse("amount is above 256 bits"));
    }
    Ok(())
}

/// Characters a memo may not carry: controls, line separators, bidirectional
/// overrides and isolates, and zero-width marks. Each can make text read as
/// something it is not.
fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// Checks a memo: at most [`MAX_MEMO_BYTES`] and no unsafe character.
///
/// # Errors
///
/// [`ChatError::Refused`].
pub fn check_memo(memo: &str) -> Result<(), ChatError> {
    if memo.len() > MAX_MEMO_BYTES {
        return Err(refuse("memo is too long"));
    }
    if memo.chars().any(is_unsafe_char) {
        return Err(refuse("memo has a control or direction character"));
    }
    Ok(())
}

/// Checks a request's expiry against `now`.
fn check_expiry(expires_at: u64, now: u64) -> Result<(), ChatError> {
    if now >= expires_at {
        return Err(ChatError::Expired);
    }
    if expires_at > now.saturating_add(MAX_TTL) {
        return Err(refuse("request lives longer than the limit"));
    }
    Ok(())
}

fn put(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
    h.update(bytes);
}

fn request_digest(
    recipient: &Address,
    payee: &Address,
    expires_at: u64,
    nonce: &[u8; NONCE_LEN],
    amount: &str,
    memo: &str,
) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(REQUEST_DOMAIN);
    put(&mut h, recipient);
    put(&mut h, payee);
    put(&mut h, &expires_at.to_le_bytes());
    put(&mut h, nonce);
    put(&mut h, amount.as_bytes());
    put(&mut h, memo.as_bytes());
    *h.finalize().as_bytes()
}

fn paid_digest(payer: &Address, payee: &Address, request_id: &[u8; 32], tx_id: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(PAID_DOMAIN);
    put(&mut h, payer);
    put(&mut h, payee);
    put(&mut h, request_id);
    put(&mut h, tx_id);
    *h.finalize().as_bytes()
}

/// Takes a fixed-size array off the front of `rest`.
fn take<const N: usize>(rest: &mut &[u8]) -> Result<[u8; N], ChatError> {
    let (head, tail) = rest
        .split_first_chunk::<N>()
        .ok_or(refuse("malformed payment message"))?;
    *rest = tail;
    Ok(*head)
}

/// Takes `n` bytes off the front of `rest`.
fn take_slice<'a>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8], ChatError> {
    if rest.len() < n {
        return Err(refuse("malformed payment message"));
    }
    let (head, tail) = rest.split_at(n);
    *rest = tail;
    Ok(head)
}

fn text(bytes: &[u8]) -> Result<String, ChatError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| refuse("payment text is not UTF-8"))
}

/// A payee's request for money from one payer. The signature covers the
/// payee, the payer it was addressed to, the amount, the memo, the expiry and
/// a nonce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The payee: the signer, and the chain address the money must reach.
    pub payee: Address,
    /// Decimal base units.
    pub amount: String,
    /// The payee's claim about what the payment is for. Text, never markup.
    pub memo: String,
    /// Unix seconds after which the payer must not pay.
    pub expires_at: u64,
    /// Random, so that two requests for the same amount differ.
    pub nonce: [u8; NONCE_LEN],
    /// The payee's ML-DSA-65 signature over the digest.
    pub signature: Vec<u8>,
}

impl Request {
    /// A request from `identity` to `recipient`.
    ///
    /// # Errors
    ///
    /// The field rules, [`ChatError::Expired`] for a past expiry, entropy or
    /// signing failure.
    pub fn sign(
        identity: &Identity,
        recipient: &Address,
        amount: &str,
        memo: &str,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, ChatError> {
        check_amount(amount)?;
        check_memo(memo)?;
        check_expiry(expires_at, now)?;
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| ChatError::Entropy)?;
        let payee = identity.address();
        let digest = request_digest(recipient, &payee, expires_at, &nonce, amount, memo);
        let signature = identity.sign(&digest)?;
        Ok(Self {
            payee,
            amount: amount.to_owned(),
            memo: memo.to_owned(),
            expires_at,
            nonce,
            signature,
        })
    }

    /// The request id as `recipient` sees it: the signed digest.
    #[must_use]
    pub fn id(&self, recipient: &Address) -> [u8; 32] {
        request_digest(
            recipient,
            &self.payee,
            self.expires_at,
            &self.nonce,
            &self.amount,
            &self.memo,
        )
    }

    /// Checks a request received from `from` (the session peer, whose identity
    /// key is `signer_key`), addressed to `recipient`, at `now`. The payee must
    /// be the sender: a sender may not relay someone else's request.
    ///
    /// # Errors
    ///
    /// [`ChatError::BadSignature`] for a mismatch, [`ChatError::Expired`], or a
    /// refused field.
    pub fn verify(
        &self,
        from: &Address,
        signer_key: &[u8],
        recipient: &Address,
        now: u64,
    ) -> Result<(), ChatError> {
        check_amount(&self.amount)?;
        check_memo(&self.memo)?;
        check_expiry(self.expires_at, now)?;
        if self.payee != *from || signer_key.is_empty() || address_of(signer_key) != *from {
            return Err(ChatError::BadSignature);
        }
        suite::verify(
            SuiteId::MlDsa65,
            signer_key,
            &self.id(recipient),
            &self.signature,
        )
        .map_err(|_| ChatError::BadSignature)
    }

    /// The plaintext: [`MAGIC`], then the fields, then the signature.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let amount = self.amount.as_bytes();
        let memo = self.memo.as_bytes();
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&self.payee);
        out.extend_from_slice(&self.expires_at.to_le_bytes());
        out.extend_from_slice(&self.nonce);
        out.push(u8::try_from(amount.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(amount);
        out.extend_from_slice(&u16::try_from(memo.len()).unwrap_or(u16::MAX).to_le_bytes());
        out.extend_from_slice(memo);
        out.extend_from_slice(&self.signature);
        out
    }

    /// Reads a request plaintext. Checks the shape and the field rules; the
    /// signature and expiry are [`Request::verify`]'s job.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a malformed request.
    pub fn parse(plain: &[u8]) -> Result<Self, ChatError> {
        let mut rest = plain
            .strip_prefix(MAGIC)
            .ok_or(refuse("not a payment request"))?;
        let payee = take::<32>(&mut rest)?;
        let expires_at = u64::from_le_bytes(take::<8>(&mut rest)?);
        let nonce = take::<NONCE_LEN>(&mut rest)?;
        let amount_len = usize::from(take::<1>(&mut rest)?[0]);
        let amount = text(take_slice(&mut rest, amount_len)?)?;
        let memo_len = usize::from(u16::from_le_bytes(take::<2>(&mut rest)?));
        let memo = text(take_slice(&mut rest, memo_len)?)?;
        if rest.is_empty() {
            return Err(refuse("payment request has no signature"));
        }
        check_amount(&amount)?;
        check_memo(&memo)?;
        Ok(Self {
            payee,
            amount,
            memo,
            expires_at,
            nonce,
            signature: rest.to_vec(),
        })
    }
}

/// A payer's receipt that it paid a request. Signed by the payer's identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paid {
    /// The request this pays.
    pub request_id: [u8; 32],
    /// The transaction the wallet produced.
    pub tx_id: [u8; 32],
    /// The payer's ML-DSA-65 signature over the digest.
    pub signature: Vec<u8>,
}

impl Paid {
    /// A receipt from `identity` (the payer) to `payee`.
    ///
    /// # Errors
    ///
    /// [`ChatError::Signature`] if the signer refuses.
    pub fn sign(
        identity: &Identity,
        payee: &Address,
        request_id: [u8; 32],
        tx_id: [u8; 32],
    ) -> Result<Self, ChatError> {
        let digest = paid_digest(&identity.address(), payee, &request_id, &tx_id);
        Ok(Self {
            request_id,
            tx_id,
            signature: identity.sign(&digest)?,
        })
    }

    /// The plaintext: [`PAID_MAGIC`], the ids, the signature.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = PAID_MAGIC.to_vec();
        out.extend_from_slice(&self.request_id);
        out.extend_from_slice(&self.tx_id);
        out.extend_from_slice(&self.signature);
        out
    }

    /// Reads a receipt plaintext. Shape only.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a malformed receipt.
    pub fn parse(plain: &[u8]) -> Result<Self, ChatError> {
        let mut rest = plain
            .strip_prefix(PAID_MAGIC)
            .ok_or(refuse("not a payment receipt"))?;
        let request_id = take::<32>(&mut rest)?;
        let tx_id = take::<32>(&mut rest)?;
        if rest.is_empty() {
            return Err(refuse("payment receipt has no signature"));
        }
        Ok(Self {
            request_id,
            tx_id,
            signature: rest.to_vec(),
        })
    }

    /// Checks the receipt from `payer` (whose identity key is `signer_key`) to
    /// `payee`.
    ///
    /// # Errors
    ///
    /// [`ChatError::BadSignature`] for any mismatch.
    pub fn verify(
        &self,
        payer: &Address,
        signer_key: &[u8],
        payee: &Address,
    ) -> Result<(), ChatError> {
        if signer_key.is_empty() || address_of(signer_key) != *payer {
            return Err(ChatError::BadSignature);
        }
        let digest = paid_digest(payer, payee, &self.request_id, &self.tx_id);
        suite::verify(SuiteId::MlDsa65, signer_key, &digest, &self.signature)
            .map_err(|_| ChatError::BadSignature)
    }
}

/// Whether a session plaintext is a payment request.
#[must_use]
pub fn is_request(plain: &[u8]) -> bool {
    plain.starts_with(MAGIC)
}

/// Whether a session plaintext is a payment receipt.
#[must_use]
pub fn is_paid(plain: &[u8]) -> bool {
    plain.starts_with(PAID_MAGIC)
}

/// Whether user text would pose as a payment message.
#[must_use]
pub fn is_marked(plain: &[u8]) -> bool {
    is_request(plain) || is_paid(plain)
}

/// One output of a transaction: the recipient and the amount in base units.
pub type Output = (Address, String);

/// A transaction in a canonical block, as the node reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainTx {
    /// The transaction id.
    pub id: [u8; 32],
    /// What it pays out.
    pub outputs: Vec<Output>,
}

/// Whether `block` holds transaction `tx_id` paying `payee` at least `amount`
/// (ADR-088 decision 4). Anything not canonical is not a payment.
#[must_use]
pub fn settles_on_chain(block: &[ChainTx], tx_id: &[u8; 32], payee: &Address, amount: &str) -> bool {
    if check_amount(amount).is_err() {
        return false;
    }
    block.iter().filter(|t| t.id == *tx_id).any(|t| {
        t.outputs.iter().any(|(to, paid)| {
            to == payee && check_amount(paid).is_ok() && at_least(paid, amount)
        })
    })
}

/// Compares two canonical decimals.
fn at_least(paid: &str, due: &str) -> bool {
    paid.len() > due.len() || (paid.len() == due.len() && paid >= due)
}

/// What one chat identity has issued, settled and received (ADR-088).
/// Entries drop [`MAX_TTL`] after their request expires: a request past that
/// is refused as expired anyway.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// Requests issued and not settled, by id, with their expiry.
    #[serde(default)]
    issued: BTreeMap<[u8; 32], u64>,
    /// Settled requests, by id.
    #[serde(default)]
    settled: BTreeMap<[u8; 32], Settled>,
    /// Requests received, by id, with their expiry. A replay is refused.
    #[serde(default)]
    received: BTreeMap<[u8; 32], u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Settled {
    expires_at: u64,
    tx_id: [u8; 32],
}

impl Ledger {
    /// Records a request this identity is issuing.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] if the id is known or the ledger is full.
    pub fn issue(&mut self, id: [u8; 32], expires_at: u64, now: u64) -> Result<(), ChatError> {
        self.sweep(now);
        if self.issued.contains_key(&id) || self.settled.contains_key(&id) {
            return Err(refuse("request already issued"));
        }
        if self.issued.len() >= MAX_LEDGER {
            return Err(refuse("too many open requests"));
        }
        self.issued.insert(id, expires_at);
        Ok(())
    }

    /// Forgets an issued request whose send failed.
    pub fn withdraw(&mut self, id: &[u8; 32]) {
        self.issued.remove(id);
    }

    /// Marks an issued request paid by `tx_id`, after the chain check.
    ///
    /// # Errors
    ///
    /// [`ChatError::Replayed`] if already settled; [`ChatError::Refused`] for
    /// an unknown request or a transaction that settled another one.
    pub fn settle(&mut self, id: [u8; 32], tx_id: [u8; 32], now: u64) -> Result<(), ChatError> {
        self.sweep(now);
        if self.settled.contains_key(&id) {
            return Err(ChatError::Replayed);
        }
        let Some(&expires_at) = self.issued.get(&id) else {
            return Err(refuse("no open request with that id"));
        };
        if self.settled.values().any(|s| s.tx_id == tx_id) {
            return Err(refuse("transaction already settled another request"));
        }
        self.issued.remove(&id);
        self.settled.insert(id, Settled { expires_at, tx_id });
        Ok(())
    }

    /// Records a request received; refuses a replay.
    ///
    /// # Errors
    ///
    /// [`ChatError::Replayed`] for a request seen before, or
    /// [`ChatError::Refused`] when the ledger is full.
    pub fn remember_received(
        &mut self,
        id: [u8; 32],
        expires_at: u64,
        now: u64,
    ) -> Result<(), ChatError> {
        self.sweep(now);
        if self.received.contains_key(&id) {
            return Err(ChatError::Replayed);
        }
        if self.received.len() >= MAX_LEDGER {
            return Err(refuse("too many requests held"));
        }
        self.received.insert(id, expires_at);
        Ok(())
    }

    /// Drops entries that are past their grace window.
    pub fn sweep(&mut self, now: u64) {
        let live = |expires_at: u64| expires_at.saturating_add(MAX_TTL) > now;
        self.issued.retain(|_, e| live(*e));
        self.settled.retain(|_, s| live(s.expires_at));
        self.received.retain(|_, e| live(*e));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const PAYEE_SEED: [u8; 32] = [1; 32];
    const PAYER_SEED: [u8; 32] = [2; 32];

    fn payee() -> Identity {
        Identity::from_seed(PAYEE_SEED)
    }

    fn payer() -> Identity {
        Identity::from_seed(PAYER_SEED)
    }

    fn request() -> Request {
        Request::sign(&payee(), &payer().address(), "1000", "coffee", NOW + 60, NOW).unwrap()
    }

    #[test]
    fn a_request_round_trips_and_verifies_for_its_recipient() {
        let req = request();
        let parsed = Request::parse(&req.encode()).unwrap();
        assert_eq!(parsed, req);
        let key = payee().public_key().to_vec();
        assert!(
            parsed
                .verify(&payee().address(), &key, &payer().address(), NOW)
                .is_ok()
        );
    }

    #[test]
    fn an_address_swap_fails_the_signature() {
        let mut bytes = request().encode();
        bytes[MAGIC.len()] ^= 1; // first byte of the payee address
        let swapped = Request::parse(&bytes).unwrap();
        let key = payee().public_key().to_vec();
        assert_eq!(
            swapped.verify(&swapped.payee, &key, &payer().address(), NOW),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn a_request_bound_to_another_recipient_is_refused() {
        let mallory = Identity::from_seed([9; 32]);
        let req = Request::sign(&payee(), &mallory.address(), "5", "x", NOW + 60, NOW).unwrap();
        let key = payee().public_key().to_vec();
        assert_eq!(
            req.verify(&req.payee, &key, &payer().address(), NOW),
            Err(ChatError::BadSignature),
            "bound to a recipient other than the one checking"
        );
    }

    #[test]
    fn a_relayed_request_is_refused_when_its_sender_is_not_the_payee() {
        let req = request();
        let key = payee().public_key().to_vec();
        let relay = Identity::from_seed([8; 32]).address();
        assert_eq!(
            req.verify(&relay, &key, &payer().address(), NOW),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn a_request_from_another_identity_key_is_refused() {
        let req = request();
        let other = payer();
        assert_eq!(
            req.verify(&req.payee, other.public_key(), &payer().address(), NOW),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn amount_tampering_fails_the_signature() {
        let mut bytes = request().encode();
        let amount_at = MAGIC.len() + 32 + 8 + NONCE_LEN + 1;
        bytes[amount_at] = b'9'; // "1000" becomes "9000"
        let tampered = Request::parse(&bytes).unwrap();
        let key = payee().public_key().to_vec();
        assert_eq!(
            tampered.verify(&tampered.payee, &key, &payer().address(), NOW),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn malformed_amounts_are_refused() {
        for bad in ["", "0", "007", "12a", " 5", "-1"] {
            assert!(check_amount(bad).is_err(), "{bad:?}");
        }
        let too_many = "1".repeat(MAX_AMOUNT_DIGITS + 1);
        assert!(check_amount(&too_many).is_err(), "79 digits");
        assert!(check_amount(U256_MAX).is_ok(), "the 256-bit maximum");
        let above = format!("{}9", &U256_MAX[..MAX_AMOUNT_DIGITS - 1]);
        assert!(check_amount(&above).is_err(), "above the maximum");
        assert!(check_amount("1").is_ok());
    }

    #[test]
    fn memo_with_controls_or_direction_marks_is_refused() {
        for bad in ["a\nb", "x\u{202E}evil", "x\u{200B}", "tab\there", "\u{2066}"] {
            assert!(check_memo(bad).is_err(), "{bad:?}");
        }
        assert!(check_memo(&"m".repeat(MAX_MEMO_BYTES + 1)).is_err());
        assert!(check_memo("coffee, thanks").is_ok());
        assert!(check_memo("").is_ok(), "an empty memo is fine");
    }

    #[test]
    fn expired_and_too_long_requests_are_refused() {
        let key = payee().public_key().to_vec();
        assert_eq!(
            Request::sign(&payee(), &payer().address(), "5", "", NOW, NOW),
            Err(ChatError::Expired)
        );
        assert!(
            Request::sign(&payee(), &payer().address(), "5", "", NOW + MAX_TTL + 1, NOW).is_err()
        );
        let req = request();
        assert_eq!(
            req.verify(&req.payee, &key, &payer().address(), NOW + 61),
            Err(ChatError::Expired)
        );
    }

    #[test]
    fn a_paid_receipt_round_trips_and_verifies_for_its_payee() {
        let receipt = Paid::sign(&payer(), &payee().address(), [4; 32], [5; 32]).unwrap();
        let parsed = Paid::parse(&receipt.encode()).unwrap();
        assert_eq!(parsed, receipt);
        assert!(
            parsed
                .verify(&payer().address(), payer().public_key(), &payee().address())
                .is_ok()
        );
        let other = Identity::from_seed([7; 32]).address();
        assert_eq!(
            parsed.verify(&payer().address(), payer().public_key(), &other),
            Err(ChatError::BadSignature),
            "a receipt for another payee"
        );
    }

    #[test]
    fn a_receipt_signed_by_someone_else_is_refused() {
        let forged = Paid::sign(&payee(), &payee().address(), [4; 32], [5; 32]).unwrap();
        assert_eq!(
            forged.verify(&payer().address(), payee().public_key(), &payee().address()),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn a_ledger_refuses_a_replayed_request_and_receipt() {
        let mut ledger = Ledger::default();
        ledger.remember_received([1; 32], NOW + 60, NOW).unwrap();
        assert_eq!(
            ledger.remember_received([1; 32], NOW + 60, NOW),
            Err(ChatError::Replayed)
        );

        ledger.issue([2; 32], NOW + 60, NOW).unwrap();
        ledger.settle([2; 32], [9; 32], NOW).unwrap();
        assert_eq!(
            ledger.settle([2; 32], [8; 32], NOW),
            Err(ChatError::Replayed),
            "a replayed receipt"
        );
    }

    #[test]
    fn one_transaction_cannot_settle_two_requests() {
        let mut ledger = Ledger::default();
        ledger.issue([1; 32], NOW + 60, NOW).unwrap();
        ledger.issue([2; 32], NOW + 60, NOW).unwrap();
        ledger.settle([1; 32], [7; 32], NOW).unwrap();
        assert!(ledger.settle([2; 32], [7; 32], NOW).is_err());
    }

    #[test]
    fn a_receipt_for_an_unknown_request_is_refused() {
        let mut ledger = Ledger::default();
        assert!(ledger.settle([3; 32], [7; 32], NOW).is_err());
    }

    #[test]
    fn a_payment_must_reach_the_payee_in_full() {
        let me = payee().address();
        let tx = [6; 32];
        let to_me = |amount: &str| ChainTx {
            id: tx,
            outputs: vec![(me, amount.to_owned())],
        };
        assert!(settles_on_chain(&[to_me("1000")], &tx, &me, "1000"));
        assert!(settles_on_chain(&[to_me("1001")], &tx, &me, "1000"), "overpayment");
        assert!(!settles_on_chain(&[to_me("999")], &tx, &me, "1000"), "short");
        assert!(settles_on_chain(&[to_me("10000")], &tx, &me, "999"), "a longer number is larger");
        assert!(!settles_on_chain(&[to_me("1000")], &[1; 32], &me, "1000"), "another tx");
        let to_other = ChainTx {
            id: tx,
            outputs: vec![(payer().address(), "1000".to_owned())],
        };
        assert!(!settles_on_chain(&[to_other], &tx, &me, "1000"), "paid someone else");
        assert!(!settles_on_chain(&[to_me("1000")], &tx, &me, "0"), "invalid amount");
    }

    #[test]
    fn markers_are_recognised_and_separate() {
        assert!(is_request(&request().encode()));
        assert!(!is_paid(&request().encode()));
        assert!(is_marked(b"\0maya-chat paid v1\0x"));
        assert!(!is_marked(b"hello"));
    }
}
