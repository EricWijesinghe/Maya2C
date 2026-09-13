//! The worker: observe both chains, decide, sign, journal.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use custom_l1_node::core::TxKind;
use custom_l1_node::core::htlc_payload::{HtlcClaim, HtlcLock, HtlcRefund};
use custom_l1_node::crypto::hybrid::HybridSigningKey;
use custom_l1_node::state::htlc::derive_lock_id;
use maya_htlc_lattice::{Address, Commitment, CommitmentId, LatticeSecret};

use crate::chain::{LockState, LockView, SwapChain};
use crate::error::{Result, WatcherError};
use crate::journal::Journal;
use crate::policy::{Action, BlockRate, ChainPoint, Margins, Observation, check_pairing, decide};
use crate::submit::{Pending, Purpose};
use crate::swap::{ChainSide, Leg, LockId, Phase, Role, Swap};

/// A responder's answer to an initiator's lock.
#[derive(Clone, Debug)]
pub struct RespondRequest {
    /// The initiator's lock on the Maya chain, which pays this watcher.
    pub inbound_lock_id: LockId,
    /// The commitment, as the initiator sent it. Checked against the lock.
    pub commitment: Commitment,
    /// The least the inbound lock must escrow.
    pub min_inbound_amount: u64,
    /// Whom this watcher's lock on the counterparty chain pays.
    pub outbound_recipient: Address,
    /// What it escrows.
    pub outbound_amount: u64,
    /// Its expiry on the counterparty chain.
    pub outbound_expiry: u64,
    /// Upper bound on Maya blocks per counterparty block.
    pub rate: BlockRate,
}

/// The responder's lock, as an initiator learns of it.
#[derive(Clone, Copy, Debug)]
pub struct InitiatedSwap {
    /// The commitment the initiator locked under.
    pub commitment_id: CommitmentId,
    /// The responder's lock on the counterparty chain, which pays this watcher.
    pub inbound_lock_id: LockId,
    /// The least it must escrow.
    pub min_inbound_amount: u64,
    /// Upper bound on Maya blocks per counterparty block.
    pub rate: BlockRate,
}

/// What one tick did.
#[derive(Debug, Default)]
pub struct StepReport {
    /// Every decision, including alerts.
    pub actions: Vec<(CommitmentId, Action)>,
    /// Decisions that could not be carried out, and why.
    pub failures: Vec<(CommitmentId, String)>,
}

/// A watcher over one Maya chain and one counterparty chain.
pub struct Worker {
    maya: Arc<dyn SwapChain>,
    counterparty: Arc<dyn SwapChain>,
    key: HybridSigningKey,
    margins: Margins,
    journal: Journal,
    secrets: BTreeMap<CommitmentId, LatticeSecret>,
    pending: Pending,
}

impl Worker {
    /// A worker signing with `key`.
    #[must_use]
    pub fn new(
        maya: Arc<dyn SwapChain>,
        counterparty: Arc<dyn SwapChain>,
        key: HybridSigningKey,
        margins: Margins,
        journal: Journal,
    ) -> Self {
        Self {
            maya,
            counterparty,
            key,
            margins,
            journal,
            secrets: BTreeMap::new(),
            pending: Pending::default(),
        }
    }

    /// The address locks must pay for this watcher to claim them.
    #[must_use]
    pub fn address(&self) -> Address {
        self.key.address()
    }

    /// The journal.
    #[must_use]
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Supplies an initiator's secret, as after a restart.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Refused`] if the secret produces no valid commitment.
    pub fn add_secret(&mut self, secret: LatticeSecret) -> Result<CommitmentId> {
        let id = secret
            .commitment()
            .map_err(|e| WatcherError::Refused(e.to_string()))?
            .id();
        self.secrets.insert(id, secret);
        Ok(id)
    }

    fn chain(&self, side: ChainSide) -> Arc<dyn SwapChain> {
        match side {
            ChainSide::Maya => Arc::clone(&self.maya),
            ChainSide::Counterparty => Arc::clone(&self.counterparty),
        }
    }

    /// Initiates a swap: locks on the Maya chain under the secret's commitment
    /// and starts watching for the refund at once.
    ///
    /// # Errors
    ///
    /// [`WatcherError::DuplicateSwap`] for a secret already used, and any
    /// journal, signing or RPC failure.
    pub async fn initiate(
        &mut self,
        secret: LatticeSecret,
        recipient: Address,
        amount: u64,
        expiry_height: u64,
    ) -> Result<LockId> {
        let commitment = secret
            .commitment()
            .map_err(|e| WatcherError::Refused(e.to_string()))?;
        let lock = HtlcLock {
            recipient,
            amount,
            expiry_height,
            commitment,
        };
        let lock_id = self.fund(ChainSide::Maya, lock, Role::Initiator, None).await?;
        self.add_secret(secret)?;
        Ok(lock_id)
    }

    /// Records and checks the responder's lock on an initiated swap.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Refused`] if the lock is missing or does not pay this
    /// watcher under the swap's commitment, and [`WatcherError::Pairing`] if
    /// its expiry leaves too little margin to reveal and claim.
    pub async fn accept_response(&mut self, response: InitiatedSwap) -> Result<()> {
        let swap = self
            .journal
            .get(&response.commitment_id)
            .cloned()
            .ok_or_else(|| WatcherError::Refused("no swap under that commitment".to_owned()))?;
        let inbound = self
            .counterparty
            .lock(&response.inbound_lock_id)
            .await?
            .ok_or_else(|| WatcherError::Refused("the responder's lock is not on chain".to_owned()))?;
        self.check_inbound(&inbound, &response.commitment_id, response.min_inbound_amount)?;

        let long = ChainPoint {
            tip: self.maya.tip_height().await?,
            expiry_height: swap.outbound.expiry_height,
            confirmations: self.margins.maya_confirmations,
        };
        let short = ChainPoint {
            tip: self.counterparty.tip_height().await?,
            expiry_height: inbound.expiry_height,
            confirmations: self.margins.counterparty_confirmations,
        };
        check_pairing(long, short, response.rate, self.margins.submission_blocks)?;

        let leg = Leg {
            side: ChainSide::Counterparty,
            lock_id: response.inbound_lock_id,
            expiry_height: inbound.expiry_height,
        };
        self.journal.pair(&response.commitment_id, leg)
    }

    /// Answers an initiator's lock with this watcher's own, after checking the
    /// initiator's lock pays this watcher and the timelocks pair safely.
    ///
    /// # Errors
    ///
    /// As [`Worker::accept_response`], plus [`WatcherError::DuplicateSwap`].
    pub async fn respond(&mut self, request: RespondRequest) -> Result<LockId> {
        let commitment_id = request.commitment.id();
        if self.journal.get(&commitment_id).is_some() {
            return Err(WatcherError::DuplicateSwap(hex::encode(commitment_id)));
        }
        let inbound = self
            .maya
            .lock(&request.inbound_lock_id)
            .await?
            .ok_or_else(|| WatcherError::Refused("the initiator's lock is not on chain".to_owned()))?;
        self.check_inbound(&inbound, &commitment_id, request.min_inbound_amount)?;

        let long = ChainPoint {
            tip: self.maya.tip_height().await?,
            expiry_height: inbound.expiry_height,
            confirmations: self.margins.maya_confirmations,
        };
        let short = ChainPoint {
            tip: self.counterparty.tip_height().await?,
            expiry_height: request.outbound_expiry,
            confirmations: self.margins.counterparty_confirmations,
        };
        check_pairing(long, short, request.rate, self.margins.submission_blocks)?;

        let leg = Leg {
            side: ChainSide::Maya,
            lock_id: request.inbound_lock_id,
            expiry_height: inbound.expiry_height,
        };
        let lock = HtlcLock {
            recipient: request.outbound_recipient,
            amount: request.outbound_amount,
            expiry_height: request.outbound_expiry,
            commitment: request.commitment,
        };
        self.fund(ChainSide::Counterparty, lock, Role::Responder, Some(leg))
            .await
    }

    /// Journals a swap, then broadcasts the lock that funds it — in that order.
    async fn fund(
        &mut self,
        side: ChainSide,
        lock: HtlcLock,
        role: Role,
        inbound: Option<Leg>,
    ) -> Result<LockId> {
        let chain = self.chain(side);
        self.pending.refresh(side, chain.as_ref(), &self.key).await?;
        let lock_id = derive_lock_id(&self.key.address(), self.pending.next_nonce(side)?);
        let commitment_id = lock.commitment.id();
        self.journal.insert(Swap {
            commitment_id,
            role,
            inbound,
            outbound: Leg {
                side,
                lock_id,
                expiry_height: lock.expiry_height,
            },
            phase: Phase::Active,
        })?;
        self.pending
            .submit(
                side,
                chain.as_ref(),
                &self.key,
                Purpose::Lock(commitment_id),
                TxKind::HtlcLock(Box::new(lock)),
            )
            .await?;
        Ok(lock_id)
    }

    fn check_inbound(
        &self,
        inbound: &LockView,
        commitment_id: &CommitmentId,
        min_amount: u64,
    ) -> Result<()> {
        let refuse = |reason: String| Err(WatcherError::Refused(reason));
        if inbound.state != LockState::Locked {
            return refuse("the inbound lock is already settled".to_owned());
        }
        if inbound.recipient != self.address() {
            return refuse("the inbound lock does not pay this watcher".to_owned());
        }
        if inbound.commitment_id != *commitment_id {
            return refuse("the inbound lock is under a different commitment".to_owned());
        }
        if inbound.amount < min_amount {
            return refuse(format!(
                "the inbound lock escrows {}, less than {min_amount}",
                inbound.amount
            ));
        }
        Ok(())
    }

    /// One tick over every active swap.
    ///
    /// # Errors
    ///
    /// An RPC failure reading either tip or nonce. A failure on one swap is
    /// reported in the [`StepReport`] and does not stop the others.
    pub async fn step(&mut self) -> Result<StepReport> {
        let maya_tip = self.maya.tip_height().await?;
        let counterparty_tip = self.counterparty.tip_height().await?;
        for side in [ChainSide::Maya, ChainSide::Counterparty] {
            let chain = self.chain(side);
            self.pending.refresh(side, chain.as_ref(), &self.key).await?;
        }

        let active: Vec<Swap> = self
            .journal
            .swaps()
            .iter()
            .filter(|swap| swap.phase == Phase::Active)
            .cloned()
            .collect();
        let mut report = StepReport::default();
        for swap in active {
            let observation = match self.observe(&swap, maya_tip, counterparty_tip).await {
                Ok(observation) => observation,
                Err(error) => {
                    report.failures.push((swap.commitment_id, error.to_string()));
                    continue;
                }
            };
            let opening = self
                .secrets
                .get(&swap.commitment_id)
                .map(LatticeSecret::opening);
            for action in decide(&swap, &observation, opening.as_ref(), &self.margins) {
                if let Err(error) = self.execute(&swap, &action).await {
                    report.failures.push((swap.commitment_id, error.to_string()));
                }
                report.actions.push((swap.commitment_id, action));
            }
        }
        Ok(report)
    }

    async fn observe(&self, swap: &Swap, maya_tip: u64, counterparty_tip: u64) -> Result<Observation> {
        let tip = |side| match side {
            ChainSide::Maya => maya_tip,
            ChainSide::Counterparty => counterparty_tip,
        };
        let inbound = match swap.inbound {
            Some(leg) => self.chain(leg.side).lock(&leg.lock_id).await?,
            None => None,
        };
        let outbound = self
            .chain(swap.outbound.side)
            .lock(&swap.outbound.lock_id)
            .await?;
        let inbound_side = swap.inbound.map_or(swap.outbound.side, |leg| leg.side);
        Ok(Observation {
            inbound_tip: tip(inbound_side),
            outbound_tip: tip(swap.outbound.side),
            inbound,
            outbound,
        })
    }

    async fn execute(&mut self, swap: &Swap, action: &Action) -> Result<()> {
        let id = swap.commitment_id;
        let (side, purpose, kind) = match action {
            Action::Claim {
                side,
                lock_id,
                opening,
            } => (
                *side,
                Purpose::Claim(id),
                TxKind::HtlcClaim(Box::new(HtlcClaim {
                    lock_id: *lock_id,
                    opening: opening.clone(),
                })),
            ),
            Action::Refund { side, lock_id } => (
                *side,
                Purpose::Refund(id),
                TxKind::HtlcRefund(HtlcRefund { lock_id: *lock_id }),
            ),
            Action::Finish(outcome) => {
                self.secrets.remove(&id);
                return self.journal.finish(&id, *outcome);
            }
            Action::Alert(_) => return Ok(()),
        };
        if self.pending.in_flight(purpose) {
            return Ok(());
        }
        let chain = self.chain(side);
        self.pending
            .submit(side, chain.as_ref(), &self.key, purpose, kind)
            .await
    }

    /// Steps every `poll` until `shutdown` turns true, handing each tick's
    /// result to `on_tick`.
    ///
    /// # Errors
    ///
    /// Never from a tick — those go to `on_tick` — only if the loop itself
    /// cannot continue.
    pub async fn run(
        &mut self,
        poll: Duration,
        mut shutdown: watch::Receiver<bool>,
        mut on_tick: impl FnMut(Result<StepReport>),
    ) -> Result<()> {
        let mut interval = tokio::time::interval(poll);
        loop {
            tokio::select! {
                _ = interval.tick() => on_tick(self.step().await),
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
            }
        }
    }
}
