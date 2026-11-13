//! Payment requests and payment receipts (ADR-088).
//!
//! A payee asks for money with a [`Request`] signed by its chat identity. The
//! payer's wallet pays, and the payer's chat says so with a [`Paid`] signed by
//! its identity. Neither message moves money or holds a wallet key: the chain
//! is the authority. The payee settles only with a finalized transaction from
//! the payer, found on its own chain by [`chain::find_final`], and the
//! [`ledger::Ledger`] checks every binding before it records the settlement.
//!
//! Both messages sign the chain tag (the genesis block id, as `get_chain_info`
//! serves it), so a request or receipt made for one chain is refused on
//! another.

pub mod chain;
pub mod ledger;

pub use ledger::{ChainTx, FinalTx, Ledger, Output};

use maya_crypto_pq::suite::{self, SuiteId};

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
/// Blocks below the finalized height the payee scans for a payment. A policy
/// bound on RPC work per check, not a measurement.
pub const MAX_SCAN_BLOCKS: u64 = 10_000;
/// Requests a ledger holds at once, in each direction.
pub const MAX_LEDGER: usize = 4_096;
/// Requests one contact may hold open in a ledger at once, in each direction,
/// so one contact cannot fill [`MAX_LEDGER`] for everyone else.
pub const MAX_PER_PEER: usize = 64;

/// The chain a payment is for: the genesis block id, which the chain's own
/// signatures commit to (ADR-036).
pub type ChainTag = [u8; 32];

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

/// Characters a memo may not carry: controls, every Unicode format (`Cf`)
/// character, line and paragraph separators, variation selectors and the
/// Hangul fillers. Each can make text read as something it is not, or hide
/// itself next to the amount.
fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{115F}'..='\u{1160}'
                | '\u{17B4}'..='\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}

/// Checks a memo: at most [`MAX_MEMO_BYTES`] and no unsafe character. A UI
/// still renders the amount from the signed field, never beside memo text.
///
/// # Errors
///
/// [`ChatError::Refused`].
pub fn check_memo(memo: &str) -> Result<(), ChatError> {
    if memo.len() > MAX_MEMO_BYTES {
        return Err(refuse("memo is too long"));
    }
    if memo.chars().any(is_unsafe_char) {
        return Err(refuse("memo has a control, format or direction character"));
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

/// Checks the account a request asks to be paid at, for a request signed by
/// `signer`.
///
/// Today the only account allowed is the signer's own chat address, which is
/// also its chain account (ADR-088 Context). That puts received money under
/// the chat identity key; whether to accept that or require a wallet-signed
/// binding ("chat id X may request to wallet account A") is an open design
/// decision (PR #441 review, H2). A binding would be checked here and
/// nowhere else: `pay_to` is already a separate signed field.
fn check_pay_to(signer: &Address, pay_to: &Address) -> Result<(), ChatError> {
    if signer == pay_to {
        Ok(())
    } else {
        Err(refuse("a request names only its signer's account"))
    }
}

fn put(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
    h.update(bytes);
}

/// What a payee asks for. The signed fields of a [`Request`] other than the
/// parties and the nonce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terms<'a> {
    /// The chain the payment is on.
    pub chain: ChainTag,
    /// The account the money must reach. See [`Request::pay_to`].
    pub pay_to: Address,
    /// Decimal base units.
    pub amount: &'a str,
    /// What the payment is for. Text, never markup.
    pub memo: &'a str,
    /// Unix seconds after which the payer must not pay.
    pub expires_at: u64,
}

/// A payee's request for money from one payer. The signature covers the
/// payee, the payer it was addressed to, the chain, the receiving account,
/// the amount, the memo, the expiry and a nonce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The payee: the signer.
    pub payee: Address,
    /// The account the money must reach. Equal to `payee` until a wallet
    /// binding exists (see `check_pay_to`).
    pub pay_to: Address,
    /// The chain the payment is on.
    pub chain: ChainTag,
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
        terms: &Terms<'_>,
        now: u64,
    ) -> Result<Self, ChatError> {
        let payee = identity.address();
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| ChatError::Entropy)?;
        let mut request = Self {
            payee,
            pay_to: terms.pay_to,
            chain: terms.chain,
            amount: terms.amount.to_owned(),
            memo: terms.memo.to_owned(),
            expires_at: terms.expires_at,
            nonce,
            signature: Vec::new(),
        };
        request.check_fields(now)?;
        request.signature = identity.sign(&request.id(recipient))?;
        Ok(request)
    }

    fn check_fields(&self, now: u64) -> Result<(), ChatError> {
        check_amount(&self.amount)?;
        crate::pay_link::check_link_amount(&self.amount)?;
        check_memo(&self.memo)?;
        check_expiry(self.expires_at, now)?;
        check_pay_to(&self.payee, &self.pay_to)
    }

    /// The request id as `recipient` sees it: the signed digest.
    #[must_use]
    pub fn id(&self, recipient: &Address) -> [u8; 32] {
        let mut h = blake3::Hasher::new_derive_key(REQUEST_DOMAIN);
        put(&mut h, recipient);
        put(&mut h, &self.payee);
        put(&mut h, &self.pay_to);
        put(&mut h, &self.chain);
        put(&mut h, &self.expires_at.to_le_bytes());
        put(&mut h, &self.nonce);
        put(&mut h, self.amount.as_bytes());
        put(&mut h, self.memo.as_bytes());
        *h.finalize().as_bytes()
    }

    /// Checks a request received from `from` (the session peer, whose identity
    /// key is `signer_key`), addressed to `recipient`, for `chain`, at `now`.
    /// The payee must be the sender: a sender may not relay someone else's
    /// request.
    ///
    /// # Errors
    ///
    /// [`ChatError::BadSignature`] for a mismatch, [`ChatError::Expired`], or a
    /// refused field, including a request for another chain.
    pub fn verify(
        &self,
        from: &Address,
        signer_key: &[u8],
        recipient: &Address,
        chain: &ChainTag,
        now: u64,
    ) -> Result<(), ChatError> {
        self.check_fields(now)?;
        if self.chain != *chain {
            return Err(refuse("request is for another chain"));
        }
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
        out.extend_from_slice(&self.pay_to);
        out.extend_from_slice(&self.chain);
        out.extend_from_slice(&self.expires_at.to_le_bytes());
        out.extend_from_slice(&self.nonce);
        out.push(u8::try_from(amount.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(amount);
        out.extend_from_slice(&u16::try_from(memo.len()).unwrap_or(u16::MAX).to_le_bytes());
        out.extend_from_slice(memo);
        out.extend_from_slice(&self.signature);
        out
    }

    /// Reads a request plaintext. Checks the shape and the amount and memo
    /// rules; the signature, chain and expiry are [`Request::verify`]'s job.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] for a malformed request.
    pub fn parse(plain: &[u8]) -> Result<Self, ChatError> {
        let mut rest = plain
            .strip_prefix(MAGIC)
            .ok_or(refuse("not a payment request"))?;
        let payee = take::<32>(&mut rest)?;
        let pay_to = take::<32>(&mut rest)?;
        let chain = take::<32>(&mut rest)?;
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
            pay_to,
            chain,
            amount,
            memo,
            expires_at,
            nonce,
            signature: rest.to_vec(),
        })
    }
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

fn paid_digest(
    payer: &Address,
    payee: &Address,
    chain: &ChainTag,
    request_id: &[u8; 32],
    tx_id: &[u8; 32],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(PAID_DOMAIN);
    put(&mut h, payer);
    put(&mut h, payee);
    put(&mut h, chain);
    put(&mut h, request_id);
    put(&mut h, tx_id);
    *h.finalize().as_bytes()
}

/// A payer's receipt that it paid a request. Signed by the payer's identity,
/// over the chain as well as the ids.
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
    /// A receipt from `identity` (the payer) to `payee`, on `chain`.
    ///
    /// # Errors
    ///
    /// [`ChatError::Signature`] if the signer refuses.
    pub fn sign(
        identity: &Identity,
        payee: &Address,
        chain: &ChainTag,
        request_id: [u8; 32],
        tx_id: [u8; 32],
    ) -> Result<Self, ChatError> {
        let digest = paid_digest(&identity.address(), payee, chain, &request_id, &tx_id);
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
    /// `payee`, on `chain`.
    ///
    /// # Errors
    ///
    /// [`ChatError::BadSignature`] for any mismatch, a receipt for another
    /// chain included.
    pub fn verify(
        &self,
        payer: &Address,
        signer_key: &[u8],
        payee: &Address,
        chain: &ChainTag,
    ) -> Result<(), ChatError> {
        if signer_key.is_empty() || address_of(signer_key) != *payer {
            return Err(ChatError::BadSignature);
        }
        let digest = paid_digest(payer, payee, chain, &self.request_id, &self.tx_id);
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

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const CHAIN: ChainTag = [0xc1; 32];
    const PAYEE_SEED: [u8; 32] = [1; 32];
    const PAYER_SEED: [u8; 32] = [2; 32];

    fn payee() -> Identity {
        Identity::from_seed(PAYEE_SEED)
    }

    fn payer() -> Identity {
        Identity::from_seed(PAYER_SEED)
    }

    fn terms(amount: &str, expires_at: u64) -> Terms<'_> {
        Terms {
            chain: CHAIN,
            pay_to: payee().address(),
            amount,
            memo: "coffee",
            expires_at,
        }
    }

    fn request() -> Request {
        Request::sign(&payee(), &payer().address(), &terms("1000", NOW + 60), NOW).unwrap()
    }

    fn check(req: &Request, from: &Address, key: &[u8]) -> Result<(), ChatError> {
        req.verify(from, key, &payer().address(), &CHAIN, NOW)
    }

    #[test]
    fn a_request_round_trips_and_verifies_for_its_recipient() {
        let req = request();
        let parsed = Request::parse(&req.encode()).unwrap();
        assert_eq!(parsed, req);
        assert!(check(&parsed, &payee().address(), payee().public_key()).is_ok());
    }

    #[test]
    fn an_address_swap_fails_the_signature() {
        let mut bytes = request().encode();
        bytes[MAGIC.len()] ^= 1; // first byte of the payee address
        let swapped = Request::parse(&bytes).unwrap();
        assert!(check(&swapped, &swapped.payee, payee().public_key()).is_err());
    }

    #[test]
    fn a_swapped_receiving_account_is_refused() {
        let mut req = request();
        req.pay_to = payer().address();
        assert_eq!(
            check(&req, &payee().address(), payee().public_key()),
            Err(ChatError::Refused(
                "a request names only its signer's account"
            ))
        );
        let mut fields = terms("5", NOW + 60);
        fields.pay_to = payer().address();
        assert!(
            Request::sign(&payee(), &payer().address(), &fields, NOW).is_err(),
            "signing for another account is refused too"
        );
    }

    #[test]
    fn a_request_for_another_chain_is_refused() {
        let req = request();
        let other: ChainTag = [0xc2; 32];
        assert_eq!(
            req.verify(
                &payee().address(),
                payee().public_key(),
                &payer().address(),
                &other,
                NOW
            ),
            Err(ChatError::Refused("request is for another chain"))
        );
        let mut moved = req;
        moved.chain = other;
        assert_eq!(
            moved.verify(
                &payee().address(),
                payee().public_key(),
                &payer().address(),
                &other,
                NOW
            ),
            Err(ChatError::BadSignature),
            "rewriting the chain field breaks the signature"
        );
    }

    #[test]
    fn a_request_bound_to_another_recipient_is_refused() {
        let mallory = Identity::from_seed([9; 32]);
        let req = Request::sign(&payee(), &mallory.address(), &terms("5", NOW + 60), NOW).unwrap();
        assert_eq!(
            check(&req, &req.payee, payee().public_key()),
            Err(ChatError::BadSignature),
            "bound to a recipient other than the one checking"
        );
    }

    #[test]
    fn a_relayed_request_is_refused_when_its_sender_is_not_the_payee() {
        let req = request();
        let relay = Identity::from_seed([8; 32]).address();
        assert_eq!(
            check(&req, &relay, payee().public_key()),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn a_request_from_another_identity_key_is_refused() {
        let req = request();
        assert_eq!(
            check(&req, &req.payee, payer().public_key()),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn amount_tampering_fails_the_signature() {
        let mut bytes = request().encode();
        let amount_at = MAGIC.len() + 32 * 3 + 8 + NONCE_LEN + 1;
        bytes[amount_at] = b'9'; // "1000" becomes "9000"
        let tampered = Request::parse(&bytes).unwrap();
        assert_eq!(tampered.amount, "9000");
        assert_eq!(
            check(&tampered, &tampered.payee, payee().public_key()),
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
    fn an_amount_the_pay_link_cannot_carry_is_refused_at_signing() {
        assert!(
            Request::sign(
                &payee(),
                &payer().address(),
                &terms(U256_MAX, NOW + 60),
                NOW
            )
            .is_err(),
            "a 78-digit amount needs more than the link's 40 whole-coin characters"
        );
    }

    #[test]
    fn memo_with_controls_format_or_direction_marks_is_refused() {
        for bad in [
            "a\nb",
            "x\u{202E}evil",
            "x\u{200B}",
            "tab\there",
            "\u{2066}",
            "\u{061C}12",
            "\u{2060}",
            "\u{2064}",
            "\u{180E}",
            "so\u{00AD}ft",
            "\u{FE0F}",
            "\u{E0041}",
            "\u{3164}",
            "\u{115F}",
        ] {
            assert!(check_memo(bad).is_err(), "{bad:?}");
        }
        assert!(check_memo(&"m".repeat(MAX_MEMO_BYTES + 1)).is_err());
        assert!(check_memo("coffee, thanks").is_ok());
        assert!(check_memo("café 커피").is_ok(), "ordinary letters pass");
        assert!(check_memo("").is_ok(), "an empty memo is fine");
    }

    #[test]
    fn expired_and_too_long_requests_are_refused() {
        assert_eq!(
            Request::sign(&payee(), &payer().address(), &terms("5", NOW), NOW),
            Err(ChatError::Expired)
        );
        let long = terms("5", NOW + MAX_TTL + 1);
        assert!(Request::sign(&payee(), &payer().address(), &long, NOW).is_err());
        let req = request();
        assert_eq!(
            req.verify(
                &req.payee,
                payee().public_key(),
                &payer().address(),
                &CHAIN,
                NOW + 61
            ),
            Err(ChatError::Expired)
        );
    }

    #[test]
    fn a_paid_receipt_round_trips_and_verifies_for_its_payee_and_chain() {
        let receipt = Paid::sign(&payer(), &payee().address(), &CHAIN, [4; 32], [5; 32]).unwrap();
        let parsed = Paid::parse(&receipt.encode()).unwrap();
        assert_eq!(parsed, receipt);
        let signer = payer();
        let key = signer.public_key();
        assert!(
            parsed
                .verify(&payer().address(), key, &payee().address(), &CHAIN)
                .is_ok()
        );
        let other = Identity::from_seed([7; 32]).address();
        assert_eq!(
            parsed.verify(&payer().address(), key, &other, &CHAIN),
            Err(ChatError::BadSignature),
            "a receipt for another payee"
        );
        assert_eq!(
            parsed.verify(&payer().address(), key, &payee().address(), &[0xc2; 32]),
            Err(ChatError::BadSignature),
            "a receipt for another chain"
        );
    }

    #[test]
    fn a_receipt_signed_by_someone_else_is_refused() {
        let forged = Paid::sign(&payee(), &payee().address(), &CHAIN, [4; 32], [5; 32]).unwrap();
        assert_eq!(
            forged.verify(
                &payer().address(),
                payee().public_key(),
                &payee().address(),
                &CHAIN
            ),
            Err(ChatError::BadSignature)
        );
    }

    #[test]
    fn markers_are_recognised_and_separate() {
        assert!(is_request(&request().encode()));
        assert!(!is_paid(&request().encode()));
        assert!(is_marked(b"\0maya-chat paid v1\0x"));
        assert!(!is_marked(b"hello"));
    }
}
