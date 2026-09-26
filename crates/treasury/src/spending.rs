//! Treasury spending: a per-epoch limit, staged approval, a public ledger.
//!
//! A spend passes through `Proposed → Approved(k of stages) → Executed`. Each
//! stage is a distinct approver role (e.g. grants committee, then security
//! council, then a timelock); a spend executes only after every stage in order
//! and only if the epoch's remaining limit covers it. Every transition is
//! appended to the ledger, which is what "transparent on-chain reporting"
//! reads.

/// An approval stage (role), in the order they must sign.
pub type Stage = u8;

/// State of one spend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Waiting on stage `next`.
    Pending {
        /// The next stage that must approve.
        next: Stage,
    },
    /// Paid out at `height`.
    Executed {
        /// Height paid.
        height: u64,
    },
    /// Rejected by `stage`.
    Rejected {
        /// The stage that said no.
        stage: Stage,
    },
}

/// A requested payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spend {
    /// Recipient address.
    pub recipient: [u8; 32],
    /// Amount.
    pub amount: u64,
    /// Human-readable purpose, published as-is.
    pub memo: String,
    /// Where it is.
    pub status: Status,
}

/// One ledger entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Height.
    pub height: u64,
    /// Spend index.
    pub spend: usize,
    /// What happened.
    pub event: &'static str,
}

/// Why a transition was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpendError {
    /// No such spend.
    Unknown,
    /// Not this stage's turn, or the spend is closed.
    OutOfOrder,
    /// The epoch's limit would be exceeded.
    OverEpochLimit,
    /// The treasury balance is short.
    Insufficient,
}

/// The treasury.
pub struct Treasury {
    /// Balance held.
    pub balance: u64,
    epoch_blocks: u64,
    epoch_limit: u64,
    stages: Stage,
    spent_epoch: (u64, u64),
    spends: Vec<Spend>,
    ledger: Vec<Entry>,
}

impl Treasury {
    /// A treasury paying at most `epoch_limit` per `epoch_blocks`, with
    /// `stages` approvals required per spend.
    #[must_use]
    pub fn new(balance: u64, epoch_blocks: u64, epoch_limit: u64, stages: Stage) -> Self {
        Self {
            balance,
            epoch_blocks: epoch_blocks.max(1),
            epoch_limit,
            stages: stages.max(1),
            spent_epoch: (0, 0),
            spends: Vec::new(),
            ledger: Vec::new(),
        }
    }

    /// Proposes a spend; returns its index.
    pub fn propose(&mut self, recipient: [u8; 32], amount: u64, memo: &str, height: u64) -> usize {
        self.spends.push(Spend {
            recipient,
            amount,
            memo: memo.to_string(),
            status: Status::Pending { next: 0 },
        });
        let i = self.spends.len() - 1;
        self.ledger.push(Entry {
            height,
            spend: i,
            event: "proposed",
        });
        i
    }

    /// `stage` approves spend `i`; the last stage executes it.
    ///
    /// # Errors
    ///
    /// [`SpendError`] if out of order, over the epoch limit, or underfunded.
    pub fn approve(&mut self, i: usize, stage: Stage, height: u64) -> Result<&Status, SpendError> {
        let (amount, next) = match self.spends.get(i).ok_or(SpendError::Unknown)? {
            Spend {
                amount,
                status: Status::Pending { next },
                ..
            } if *next == stage => (*amount, *next),
            _ => return Err(SpendError::OutOfOrder),
        };
        if next + 1 < self.stages {
            self.spends[i].status = Status::Pending { next: next + 1 };
            self.ledger.push(Entry {
                height,
                spend: i,
                event: "approved",
            });
            return Ok(&self.spends[i].status);
        }
        let epoch = height / self.epoch_blocks;
        let spent = if self.spent_epoch.0 == epoch {
            self.spent_epoch.1
        } else {
            0
        };
        if spent.saturating_add(amount) > self.epoch_limit {
            return Err(SpendError::OverEpochLimit);
        }
        if amount > self.balance {
            return Err(SpendError::Insufficient);
        }
        self.balance -= amount;
        self.spent_epoch = (epoch, spent + amount);
        self.spends[i].status = Status::Executed { height };
        self.ledger.push(Entry {
            height,
            spend: i,
            event: "executed",
        });
        Ok(&self.spends[i].status)
    }

    /// `stage` rejects spend `i`.
    ///
    /// # Errors
    ///
    /// [`SpendError::OutOfOrder`] if it is not that stage's turn.
    pub fn reject(&mut self, i: usize, stage: Stage, height: u64) -> Result<(), SpendError> {
        match self.spends.get(i).ok_or(SpendError::Unknown)? {
            Spend {
                status: Status::Pending { next },
                ..
            } if *next == stage => {}
            _ => return Err(SpendError::OutOfOrder),
        }
        self.spends[i].status = Status::Rejected { stage };
        self.ledger.push(Entry {
            height,
            spend: i,
            event: "rejected",
        });
        Ok(())
    }

    /// The public ledger.
    #[must_use]
    pub fn ledger(&self) -> &[Entry] {
        &self.ledger
    }

    /// A spend.
    #[must_use]
    pub fn spend(&self, i: usize) -> Option<&Spend> {
        self.spends.get(i)
    }
}
