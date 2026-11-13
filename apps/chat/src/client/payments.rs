//! The client's side of payments (ADR-088): requests and receipts in and
//! out of sessions, the payer's and payee's ledgers, and settlement.
//!
//! Save rule: every call here that returns `Ok` changed state the caller must
//! [`Client::save`] before it sends anything from the outbox or ends the pass
//! (the delivery receipt is the acknowledgement). Opened payment frames wait
//! in the saved inbox, so a crash before the caller takes them loses nothing.

use serde::{Deserialize, Serialize};

use super::{Client, Frame};
use crate::pay::ledger::Due;
use crate::pay::{self, Binding, BindingKind, ChainTag, FinalTx, Paid, Request, binding};
use crate::pay_link;
use crate::{Address, ChatError};

/// Payment frames held at once, waiting for [`Client::take_pay_frames`].
pub const MAX_PAY_INBOX: usize = 256;

/// A payment plaintext opened and checked, kept as received until taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Inbound {
    from: Address,
    #[serde(with = "serde_bytes")]
    plain: Vec<u8>,
}

impl Inbound {
    /// The frame, parsed again: it was checked on arrival.
    fn frame(&self, me: &Address) -> Result<Frame, ChatError> {
        if pay::is_request(&self.plain) {
            let request = Request::parse(&self.plain)?;
            Ok(Frame::Pay {
                from: self.from,
                id: request.id(me),
                request,
            })
        } else if binding::is_binding(&self.plain) {
            let binding = Binding::parse_frame(&self.plain)?;
            Ok(Frame::Binding {
                from: self.from,
                revoked: binding.kind == BindingKind::Revoke,
            })
        } else {
            let paid = Paid::parse(&self.plain)?;
            Ok(Frame::Paid {
                from: self.from,
                request_id: paid.request_id,
                tx_id: paid.tx_id,
            })
        }
    }
}

impl Client {
    /// Sets the chain payments are for: its genesis id, as the node's
    /// `get_chain_info` reports it. Requests and receipts are refused until
    /// it is set.
    pub fn set_pay_chain(&mut self, chain: ChainTag) {
        self.pay_chain = Some(chain);
    }

    fn chain(&self) -> Result<ChainTag, ChatError> {
        self.pay_chain
            .ok_or(ChatError::Refused("no chain set for payments"))
    }

    /// Installs a binding the user's wallet signed for this identity on the
    /// payment chain. A [`BindingKind::Bind`] becomes the binding requests
    /// and receipts carry ([`Client::pay_binding`]); a revocation removes
    /// it. Send either to contacts with [`Client::queue_binding`].
    ///
    /// # Errors
    ///
    /// No chain set, or the binding's refusals ([`Binding::verify`]).
    pub fn set_pay_binding(&mut self, binding: Binding, now: u64) -> Result<(), ChatError> {
        binding.verify(&self.identity.address(), &self.chain()?, now)?;
        self.pay_binding = (binding.kind == BindingKind::Bind).then_some(binding);
        Ok(())
    }

    /// The wallet binding this identity's requests and receipts carry.
    #[must_use]
    pub fn pay_binding(&self) -> Option<&Binding> {
        self.pay_binding.as_ref()
    }

    /// Sends this identity's `binding` (a new one, or a revocation) to `to`
    /// on its own, so `to` refuses every older binding from now on.
    ///
    /// # Errors
    ///
    /// A binding that is not this identity's on the payment chain, no
    /// session, or an outbox refusal.
    pub fn queue_binding(
        &mut self,
        to: Address,
        binding: &Binding,
        now: u64,
        version: u8,
    ) -> Result<[u8; 32], ChatError> {
        binding.verify(&self.identity.address(), &self.chain()?, now)?;
        self.queue_marked(to, &binding.frame(), now, version)
    }

    /// Checks and holds a payment plaintext from `from`, returning its frame.
    pub(super) fn incoming_pay(
        &mut self,
        from: Address,
        peer_key: &[u8],
        plain: &[u8],
        now: u64,
    ) -> Result<Frame, ChatError> {
        if self.pay_inbox.len() >= MAX_PAY_INBOX {
            return Err(ChatError::Refused("too many payment messages waiting"));
        }
        let chain = self.chain()?;
        let me = self.identity.address();
        if pay::is_request(plain) {
            let request = Request::parse(plain)?;
            request.verify(&from, peer_key, &me, &chain, now)?;
            self.pay.remember_received(request.id(&me), &request, now)?;
        } else if binding::is_binding(plain) {
            let binding = Binding::parse_frame(plain)?;
            binding.verify(&from, &chain, now)?;
            self.pay.observe_binding(&binding, now)?;
        } else {
            let paid = Paid::parse(plain)?;
            paid.verify(&from, peer_key, &me, &chain, now)?;
            // Only the payer the request went to may report it paid; a
            // duplicate of a settled one is refused here, not queued.
            self.pay
                .note_receipt(&paid.request_id, &from, &paid.binding, now)?;
        }
        let inbound = Inbound {
            from,
            plain: plain.to_vec(),
        };
        let frame = inbound.frame(&me)?;
        self.pay_inbox.push(inbound);
        Ok(frame)
    }

    /// Payment frames opened since the last call, in arrival order. Save
    /// after taking them, once they are handled.
    pub fn take_pay_frames(&mut self) -> Vec<Frame> {
        let me = self.identity.address();
        std::mem::take(&mut self.pay_inbox)
            .iter()
            .filter_map(|i| i.frame(&me).ok())
            .collect()
    }

    /// Sends a payment request to `to`, who must already have a session with
    /// us: a request cannot open one. `tip_height` is the chain tip now, from
    /// the payee's node; a transaction at or below it cannot pay the request.
    /// Returns the outbox id; the request id is `request.id(&to)`.
    ///
    /// # Errors
    ///
    /// A request for another chain or naming another account, no session, a
    /// ledger cap, or an outbox refusal.
    pub fn queue_request(
        &mut self,
        to: Address,
        request: &Request,
        tip_height: u64,
        now: u64,
        version: u8,
    ) -> Result<[u8; 32], ChatError> {
        if request.payee != self.identity.address() {
            return Err(ChatError::Refused(
                "a request names only its signer's account",
            ));
        }
        if request.chain != self.chain()? {
            return Err(ChatError::Refused("request is for another chain"));
        }
        let id = request.id(&to);
        self.pay.issue(id, to, request, tip_height, now)?;
        self.queue_marked(to, &request.encode(), now, version)
            .inspect_err(|_| self.pay.withdraw(&id))
    }

    /// The tip height recorded when request `id` was issued, while it is
    /// open. Search the chain from one above it ([`pay::chain::find_final`]).
    #[must_use]
    pub fn issued_height(&self, id: &[u8; 32]) -> Option<u64> {
        self.pay.issued_height(id)
    }

    /// Records that request `request_id`, reported paid by `from` with
    /// `tx_id`, is settled by `found`, the finalized transaction
    /// [`pay::chain::find_final`] returned.
    ///
    /// # Errors
    ///
    /// As [`pay::Ledger::settle`].
    pub fn settle(
        &mut self,
        from: &Address,
        request_id: [u8; 32],
        tx_id: [u8; 32],
        found: &FinalTx,
        now: u64,
    ) -> Result<(), ChatError> {
        self.pay.settle(request_id, from, tx_id, found, now)
    }

    /// The wallet link that pays received request `id`, written for
    /// `network`. Refused for a request never received, expired, or already
    /// paid: the payer's guard against paying twice.
    ///
    /// # Errors
    ///
    /// As [`pay::Ledger::due`].
    pub fn pay_link(
        &self,
        id: &[u8; 32],
        network: maya_address::Network,
        now: u64,
    ) -> Result<String, ChatError> {
        let due: Due = self.pay.due(id, now)?;
        Ok(pay_link::pay_uri(network, &due, id))
    }

    /// Sends a payment receipt to the payee `to`, over the existing session,
    /// and marks the request paid.
    ///
    /// # Errors
    ///
    /// A request never received from `to`, already paid, no chain set, no
    /// session, or an outbox refusal.
    pub fn queue_paid(
        &mut self,
        to: Address,
        paid: &Paid,
        now: u64,
        version: u8,
    ) -> Result<[u8; 32], ChatError> {
        paid.binding
            .verify_bind(&self.identity.address(), &self.chain()?, now)?;
        self.pay.mark_paid(&paid.request_id, &to, paid.tx_id)?;
        self.queue_marked(to, &paid.encode(), now, version)
            .inspect_err(|_| self.pay.unmark_paid(&paid.request_id))
    }

    /// [`Client::queue`] for the payment messages.
    fn queue_marked(
        &mut self,
        to: Address,
        text: &[u8],
        now: u64,
        version: u8,
    ) -> Result<[u8; 32], ChatError> {
        self.queue_with(to, None, text, now, version, false)
    }
}
