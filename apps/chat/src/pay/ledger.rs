//! What one chat identity has issued, settled, received and paid (ADR-088),
//! and the settlement rule that binds a transaction to one request.
//!
//! A receipt settles a request only when all of these hold (PR #441 review,
//! H1, M2, L4):
//! - the receipt comes from the payer the request was issued to;
//! - the transaction is the one the receipt names, and the payer sent it;
//! - it sits in a block above the tip height recorded when the request was
//!   issued, and at or below the finalized (checkpointed) height;
//! - it pays the request's receiving account at least the amount;
//! - no other request was settled by it.
//!
//! Callers save the client after every change here and before anything is
//! sent or acknowledged ([`crate::client::Client::save`]); state that only
//! lived in memory is lost with the process.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{ChainTag, MAX_LEDGER, MAX_PER_PEER, MAX_TTL, Request, check_amount};
use crate::{Address, ChatError};

fn refuse(why: &'static str) -> ChatError {
    ChatError::Refused(why)
}

/// One output of a transaction: the recipient and the amount in base units.
pub type Output = (Address, String);

/// A transaction as the node reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainTx {
    /// The transaction id.
    pub id: [u8; 32],
    /// The account that sent it.
    pub sender: Address,
    /// What it pays out.
    pub outputs: Vec<Output>,
}

/// A transaction found in a finalized block. Only [`super::chain::find_final`]
/// makes one, so a settlement cannot skip the chain check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalTx {
    height: u64,
    finalized: u64,
    tx: ChainTx,
}

impl FinalTx {
    pub(crate) fn new(height: u64, finalized: u64, tx: ChainTx) -> Self {
        Self {
            height,
            finalized,
            tx,
        }
    }

    /// Height of the block holding the transaction.
    #[must_use]
    pub fn height(&self) -> u64 {
        self.height
    }

    /// The finalized height the node reported when the transaction was found.
    #[must_use]
    pub fn finalized(&self) -> u64 {
        self.finalized
    }

    /// The transaction.
    #[must_use]
    pub fn tx(&self) -> &ChainTx {
        &self.tx
    }
}

/// Compares two canonical decimals.
fn at_least(paid: &str, due: &str) -> bool {
    paid.len() > due.len() || (paid.len() == due.len() && paid >= due)
}

/// A request this identity issued and has not seen settled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Issued {
    payer: Address,
    pay_to: Address,
    amount: String,
    expires_at: u64,
    /// The chain tip when the request was issued: an older transaction cannot
    /// pay it.
    height: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Settled {
    tx_id: [u8; 32],
    keep_until: u64,
}

/// A request received from a payee.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Incoming {
    payee: Address,
    pay_to: Address,
    chain: ChainTag,
    amount: String,
    expires_at: u64,
    /// The transaction the payer reported, once it has.
    paid: Option<[u8; 32]>,
}

/// What the payer's wallet must pay for one received request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    /// The account the money must reach.
    pub pay_to: Address,
    /// The chain it must be paid on.
    pub chain: ChainTag,
    /// Decimal base units.
    pub amount: String,
}

impl Due {
    /// What `request` asks for.
    #[must_use]
    pub fn of(request: &Request) -> Self {
        Self {
            pay_to: request.pay_to,
            chain: request.chain,
            amount: request.amount.clone(),
        }
    }
}

/// What one chat identity has issued, settled and received. Entries drop
/// [`MAX_TTL`] after their request expires; a settled transaction id is kept
/// for twice [`MAX_TTL`] after settling, longer than any request issued
/// before that transaction can stay open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    issued: BTreeMap<[u8; 32], Issued>,
    #[serde(default)]
    settled: BTreeMap<[u8; 32], Settled>,
    #[serde(default)]
    received: BTreeMap<[u8; 32], Incoming>,
}

impl Ledger {
    /// Records `request`, issued to `payer` with the chain tip at `height`.
    ///
    /// # Errors
    ///
    /// [`ChatError::Refused`] if the id is known or a cap is reached.
    pub fn issue(
        &mut self,
        id: [u8; 32],
        payer: Address,
        request: &Request,
        height: u64,
        now: u64,
    ) -> Result<(), ChatError> {
        self.sweep(now);
        if self.issued.contains_key(&id) || self.settled.contains_key(&id) {
            return Err(refuse("request already issued"));
        }
        if self.issued.len() >= MAX_LEDGER {
            return Err(refuse("too many open requests"));
        }
        if self.issued.values().filter(|i| i.payer == payer).count() >= MAX_PER_PEER {
            return Err(refuse("too many open requests to this contact"));
        }
        self.issued.insert(
            id,
            Issued {
                payer,
                pay_to: request.pay_to,
                amount: request.amount.clone(),
                expires_at: request.expires_at,
                height,
            },
        );
        Ok(())
    }

    /// Forgets an issued request whose send failed.
    pub fn withdraw(&mut self, id: &[u8; 32]) {
        self.issued.remove(id);
    }

    /// The tip height recorded when `id` was issued, while it is open: where
    /// the chain search for its payment starts.
    #[must_use]
    pub fn issued_height(&self, id: &[u8; 32]) -> Option<u64> {
        self.issued.get(id).map(|i| i.height)
    }

    /// Checks that a receipt for `id` from `from` could settle: the request
    /// is open and was issued to `from`.
    ///
    /// # Errors
    ///
    /// [`ChatError::Replayed`] if already settled, [`ChatError::Refused`]
    /// otherwise.
    pub fn check_receipt(&self, id: &[u8; 32], from: &Address) -> Result<(), ChatError> {
        self.open_for(id, from).map(|_| ())
    }

    fn open_for(&self, id: &[u8; 32], from: &Address) -> Result<&Issued, ChatError> {
        if self.settled.contains_key(id) {
            return Err(ChatError::Replayed);
        }
        let issued = self
            .issued
            .get(id)
            .ok_or(refuse("no open request with that id"))?;
        if issued.payer != *from {
            return Err(refuse(
                "receipt is not from the payer the request was sent to",
            ));
        }
        Ok(issued)
    }

    /// Marks request `id` paid by `tx_id`, reported by `from`, with `found`
    /// as the chain's evidence. Every binding in the module comment is
    /// checked here.
    ///
    /// # Errors
    ///
    /// [`ChatError::Replayed`] if already settled; [`ChatError::Refused`] for
    /// any binding that does not hold.
    pub fn settle(
        &mut self,
        id: [u8; 32],
        from: &Address,
        tx_id: [u8; 32],
        found: &FinalTx,
        now: u64,
    ) -> Result<(), ChatError> {
        self.sweep(now);
        let issued = self.open_for(&id, from)?;
        let tx = found.tx();
        if tx.id != tx_id {
            return Err(refuse("chain evidence is for another transaction"));
        }
        if found.height() > found.finalized() {
            return Err(refuse("transaction is not final"));
        }
        if found.height() <= issued.height {
            return Err(refuse("transaction is older than the request"));
        }
        // The payer's chat address is its chain account (ADR-088 Context);
        // a wallet binding would name the paying account here.
        if tx.sender != issued.payer {
            return Err(refuse("transaction was not sent by the payer"));
        }
        let pays = tx.outputs.iter().any(|(to, paid)| {
            *to == issued.pay_to && check_amount(paid).is_ok() && at_least(paid, &issued.amount)
        });
        if !pays {
            return Err(refuse("transaction does not pay the request in full"));
        }
        if self.settled.values().any(|s| s.tx_id == tx_id) {
            return Err(refuse("transaction already settled another request"));
        }
        self.issued.remove(&id);
        let keep_until = now.saturating_add(2 * MAX_TTL);
        self.settled.insert(id, Settled { tx_id, keep_until });
        Ok(())
    }

    /// Records a request received from its payee; refuses a replay.
    ///
    /// # Errors
    ///
    /// [`ChatError::Replayed`] for a request seen before, or
    /// [`ChatError::Refused`] when a cap is reached.
    pub fn remember_received(
        &mut self,
        id: [u8; 32],
        request: &Request,
        now: u64,
    ) -> Result<(), ChatError> {
        self.sweep(now);
        if self.received.contains_key(&id) {
            return Err(ChatError::Replayed);
        }
        if self.received.len() >= MAX_LEDGER {
            return Err(refuse("too many requests held"));
        }
        let from_payee = self
            .received
            .values()
            .filter(|r| r.payee == request.payee)
            .count();
        if from_payee >= MAX_PER_PEER {
            return Err(refuse("too many requests held from this contact"));
        }
        self.received.insert(
            id,
            Incoming {
                payee: request.payee,
                pay_to: request.pay_to,
                chain: request.chain,
                amount: request.amount.clone(),
                expires_at: request.expires_at,
                paid: None,
            },
        );
        Ok(())
    }

    /// What to pay for received request `id`, if it may still be paid: known,
    /// not expired, not already paid. The payer's guard against paying twice
    /// or paying a request it never received.
    ///
    /// # Errors
    ///
    /// [`ChatError::Expired`], [`ChatError::Replayed`] if already paid, or
    /// [`ChatError::Refused`] for an unknown id.
    pub fn due(&self, id: &[u8; 32], now: u64) -> Result<Due, ChatError> {
        let incoming = self
            .received
            .get(id)
            .ok_or(refuse("no request received with that id"))?;
        if incoming.paid.is_some() {
            return Err(ChatError::Replayed);
        }
        if now >= incoming.expires_at {
            return Err(ChatError::Expired);
        }
        Ok(Due {
            pay_to: incoming.pay_to,
            chain: incoming.chain,
            amount: incoming.amount.clone(),
        })
    }

    /// Records that received request `id`, from `payee`, was paid by `tx_id`.
    /// A receipt is still allowed after the expiry: the money has moved, and
    /// the payee must hear of it.
    ///
    /// # Errors
    ///
    /// [`ChatError::Replayed`] if already paid; [`ChatError::Refused`] for an
    /// unknown id or another payee.
    pub fn mark_paid(
        &mut self,
        id: &[u8; 32],
        payee: &Address,
        tx_id: [u8; 32],
    ) -> Result<(), ChatError> {
        let incoming = self
            .received
            .get_mut(id)
            .ok_or(refuse("no request received with that id"))?;
        if incoming.payee != *payee {
            return Err(refuse("request came from another payee"));
        }
        if incoming.paid.is_some() {
            return Err(ChatError::Replayed);
        }
        incoming.paid = Some(tx_id);
        Ok(())
    }

    /// Undoes [`Ledger::mark_paid`] when the receipt could not be queued.
    pub fn unmark_paid(&mut self, id: &[u8; 32]) {
        if let Some(incoming) = self.received.get_mut(id) {
            incoming.paid = None;
        }
    }

    /// Drops entries that are past their grace window.
    pub fn sweep(&mut self, now: u64) {
        let live = |expires_at: u64| expires_at.saturating_add(MAX_TTL) > now;
        self.issued.retain(|_, i| live(i.expires_at));
        self.settled.retain(|_, s| s.keep_until > now);
        self.received.retain(|_, r| live(r.expires_at));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;
    use crate::identity::Identity;
    use crate::pay::Terms;

    const NOW: u64 = 1_800_000_000;
    const CHAIN: ChainTag = [0xc1; 32];
    const ISSUED_AT: u64 = 100;
    const FINAL: u64 = 200;
    const TX: [u8; 32] = [0x42; 32];

    fn payee() -> Address {
        Identity::from_seed([1; 32]).address()
    }

    fn payer() -> Address {
        Identity::from_seed([2; 32]).address()
    }

    fn request(amount: &str) -> Request {
        let terms = Terms {
            chain: CHAIN,
            pay_to: payee(),
            amount,
            memo: "",
            expires_at: NOW + 60,
        };
        Request::sign(&Identity::from_seed([1; 32]), &payer(), &terms, NOW).unwrap()
    }

    fn tx(sender: Address, to: Address, amount: &str) -> ChainTx {
        ChainTx {
            id: TX,
            sender,
            outputs: vec![(to, amount.to_owned())],
        }
    }

    fn found(height: u64, tx: ChainTx) -> FinalTx {
        FinalTx::new(height, FINAL, tx)
    }

    /// A ledger with request `[1; 32]` for 1000 issued to the payer.
    fn issued() -> Ledger {
        let mut ledger = Ledger::default();
        ledger
            .issue([1; 32], payer(), &request("1000"), ISSUED_AT, NOW)
            .unwrap();
        ledger
    }

    fn settle(ledger: &mut Ledger, evidence: &FinalTx) -> Result<(), ChatError> {
        ledger.settle([1; 32], &payer(), TX, evidence, NOW)
    }

    #[test]
    fn a_final_payment_from_the_payer_settles_once() {
        let mut ledger = issued();
        let good = found(150, tx(payer(), payee(), "1000"));
        settle(&mut ledger, &good).unwrap();
        assert_eq!(settle(&mut ledger, &good), Err(ChatError::Replayed));
    }

    #[test]
    fn someone_elses_payment_to_the_payee_does_not_settle() {
        let mut ledger = issued();
        let exchange = Identity::from_seed([5; 32]).address();
        assert_eq!(
            settle(&mut ledger, &found(150, tx(exchange, payee(), "1000000"))),
            Err(refuse("transaction was not sent by the payer"))
        );
        assert!(ledger.issued_height(&[1; 32]).is_some(), "still open");
    }

    #[test]
    fn a_transaction_from_before_the_request_does_not_settle() {
        let mut ledger = issued();
        for height in [ISSUED_AT - 1, ISSUED_AT] {
            assert_eq!(
                settle(&mut ledger, &found(height, tx(payer(), payee(), "1000"))),
                Err(refuse("transaction is older than the request")),
                "height {height}"
            );
        }
    }

    #[test]
    fn an_unfinalized_transaction_does_not_settle() {
        let mut ledger = issued();
        assert_eq!(
            settle(&mut ledger, &found(FINAL + 1, tx(payer(), payee(), "1000"))),
            Err(refuse("transaction is not final"))
        );
    }

    #[test]
    fn the_evidence_must_be_for_the_named_transaction() {
        let mut ledger = issued();
        let mut other = tx(payer(), payee(), "1000");
        other.id = [7; 32];
        assert!(settle(&mut ledger, &found(150, other)).is_err());
    }

    #[test]
    fn a_short_or_misdirected_payment_does_not_settle() {
        let mut ledger = issued();
        let full = Err(refuse("transaction does not pay the request in full"));
        assert_eq!(
            settle(&mut ledger, &found(150, tx(payer(), payee(), "999"))),
            full
        );
        assert_eq!(
            settle(&mut ledger, &found(150, tx(payer(), payer(), "1000"))),
            full
        );
        assert!(
            settle(&mut ledger, &found(150, tx(payer(), payee(), "10000"))).is_ok(),
            "a longer number is larger"
        );
    }

    #[test]
    fn a_receipt_from_another_contact_does_not_settle() {
        let mut ledger = issued();
        let carol = Identity::from_seed([3; 32]).address();
        assert!(ledger.check_receipt(&[1; 32], &carol).is_err());
        assert!(
            ledger
                .settle(
                    [1; 32],
                    &carol,
                    TX,
                    &found(150, tx(carol, payee(), "1000")),
                    NOW
                )
                .is_err()
        );
        assert!(ledger.check_receipt(&[1; 32], &payer()).is_ok());
    }

    #[test]
    fn one_transaction_cannot_settle_two_requests() {
        let mut ledger = issued();
        let second = request("1000");
        ledger
            .issue([2; 32], payer(), &second, ISSUED_AT, NOW)
            .unwrap();
        let evidence = found(150, tx(payer(), payee(), "1000"));
        settle(&mut ledger, &evidence).unwrap();
        assert_eq!(
            ledger.settle([2; 32], &payer(), TX, &evidence, NOW),
            Err(refuse("transaction already settled another request"))
        );
    }

    #[test]
    fn a_settled_transaction_is_remembered_longer_than_any_request_it_could_pay() {
        let mut ledger = issued();
        settle(&mut ledger, &found(150, tx(payer(), payee(), "1000"))).unwrap();
        // A request issued at the last moment before the transaction is open
        // until at most its expiry plus the grace window.
        let late = NOW + 2 * MAX_TTL - 1;
        ledger.sweep(late);
        assert!(ledger.settled.values().any(|s| s.tx_id == TX));
    }

    #[test]
    fn a_receipt_for_an_unknown_request_is_refused() {
        let mut ledger = Ledger::default();
        let evidence = found(150, tx(payer(), payee(), "1000"));
        assert!(
            ledger
                .settle([3; 32], &payer(), TX, &evidence, NOW)
                .is_err()
        );
    }

    #[test]
    fn a_replayed_request_is_refused() {
        let mut ledger = Ledger::default();
        let req = request("5");
        ledger.remember_received([1; 32], &req, NOW).unwrap();
        assert_eq!(
            ledger.remember_received([1; 32], &req, NOW),
            Err(ChatError::Replayed)
        );
    }

    #[test]
    fn one_contact_cannot_fill_the_ledger_for_everyone() {
        let mut ledger = Ledger::default();
        let req = request("5");
        for n in 0..MAX_PER_PEER {
            let id = blake3::hash(&n.to_le_bytes()).into();
            ledger.remember_received(id, &req, NOW).unwrap();
        }
        assert_eq!(
            ledger.remember_received([0xff; 32], &req, NOW),
            Err(refuse("too many requests held from this contact"))
        );
        let mut other = req;
        other.payee = Identity::from_seed([3; 32]).address();
        assert!(
            ledger.remember_received([0xfe; 32], &other, NOW).is_ok(),
            "another contact still gets through"
        );
        let mut issuing = Ledger::default();
        for n in 0..MAX_PER_PEER {
            let id = blake3::hash(&n.to_le_bytes()).into();
            issuing.issue(id, payer(), &request("5"), 1, NOW).unwrap();
        }
        assert!(
            issuing
                .issue([0xff; 32], payer(), &request("5"), 1, NOW)
                .is_err()
        );
    }

    #[test]
    fn the_payer_cannot_pay_twice_late_or_unasked() {
        let mut ledger = Ledger::default();
        let req = request("5");
        assert!(ledger.due(&[1; 32], NOW).is_err(), "never received");
        assert!(ledger.mark_paid(&[1; 32], &payee(), TX).is_err());
        ledger.remember_received([1; 32], &req, NOW).unwrap();
        assert_eq!(ledger.due(&[1; 32], NOW + 60), Err(ChatError::Expired));
        let due = ledger.due(&[1; 32], NOW).unwrap();
        assert_eq!((due.pay_to, due.amount.as_str()), (payee(), "5"));
        assert!(
            ledger.mark_paid(&[1; 32], &payer(), TX).is_err(),
            "receipt to another payee"
        );
        ledger.mark_paid(&[1; 32], &payee(), TX).unwrap();
        assert_eq!(ledger.due(&[1; 32], NOW), Err(ChatError::Replayed));
        assert_eq!(
            ledger.mark_paid(&[1; 32], &payee(), TX),
            Err(ChatError::Replayed)
        );
    }
}
