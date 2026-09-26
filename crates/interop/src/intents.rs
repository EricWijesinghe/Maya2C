//! Cross-chain intent escrow.
//!
//! A user locks funds with an intent ("deliver 100 to Alice on chain X by
//! height D"). A solver fulfils it on the other chain and claims the escrow
//! by presenting a delivery proof, which a verifier (a light client of chain
//! X) accepts or rejects. Without an accepted proof by the deadline, the
//! user takes the funds back. Nobody's word is enough: not the solver's,
//! not a relayer's. Anyone may submit the proof, so censoring relayers cannot
//! block settlement.

/// Verifies a delivery proof for an intent: a light client of the
/// destination chain.
pub trait DeliveryVerifier {
    /// Whether `proof` shows `amount` delivered to `recipient` on the
    /// destination chain, finalized.
    fn verify(&self, intent: &Intent, proof: &[u8]) -> bool;
}

/// What the user wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intent {
    /// The user who locked funds.
    pub user: [u8; 32],
    /// Recipient on the destination chain.
    pub recipient: Vec<u8>,
    /// Amount to deliver.
    pub amount: u128,
    /// Escrowed payment to the solver (amount + fee).
    pub escrow: u128,
    /// Last height a proof is accepted.
    pub deadline: u64,
}

/// Escrow state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for a proof.
    Open,
    /// Paid to the solver.
    Settled,
    /// Returned to the user.
    Refunded,
}

/// Why an action was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentError {
    /// The intent is not open.
    Closed,
    /// The proof did not verify.
    BadProof,
    /// After the deadline: only a refund is possible.
    Expired,
    /// Before the deadline: no refund yet.
    NotExpired,
}

/// One escrowed intent.
#[derive(Clone, Debug)]
pub struct Escrow {
    /// The intent.
    pub intent: Intent,
    /// Its state.
    pub state: State,
}

impl Escrow {
    /// Opens an escrow.
    #[must_use]
    pub fn open(intent: Intent) -> Self {
        Self {
            intent,
            state: State::Open,
        }
    }

    /// A solver claims with a proof; returns the amount paid.
    ///
    /// # Errors
    ///
    /// [`IntentError`] if closed, expired, or the proof does not verify.
    pub fn claim(
        &mut self,
        proof: &[u8],
        verifier: &impl DeliveryVerifier,
        height: u64,
    ) -> Result<u128, IntentError> {
        if self.state != State::Open {
            return Err(IntentError::Closed);
        }
        if height > self.intent.deadline {
            return Err(IntentError::Expired);
        }
        if !verifier.verify(&self.intent, proof) {
            return Err(IntentError::BadProof);
        }
        self.state = State::Settled;
        Ok(self.intent.escrow)
    }

    /// The user reclaims after the deadline; returns the amount refunded.
    ///
    /// # Errors
    ///
    /// [`IntentError`] if closed or not yet expired.
    pub fn refund(&mut self, height: u64) -> Result<u128, IntentError> {
        if self.state != State::Open {
            return Err(IntentError::Closed);
        }
        if height <= self.intent.deadline {
            return Err(IntentError::NotExpired);
        }
        self.state = State::Refunded;
        Ok(self.intent.escrow)
    }
}
