//! Built-in policy modules: evaluated by the chain, not by app code.

use crate::AccountId;

/// Blocks in one policy "day" (2 s blocks).
pub const BLOCKS_PER_DAY: u64 = 43_200;

/// An account's spending policy. The default is "no limits": a normal
/// account stays instant and final; limits are opt-in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Most that may leave per policy day (0 = unlimited).
    pub daily_limit: u64,
    /// Most one recipient may receive per day (0 = unlimited).
    pub per_recipient_daily: u64,
    /// If non-empty, only these recipients may be paid.
    pub allow_list: Vec<AccountId>,
    /// Transfers above this wait `delay_blocks` and can be cancelled
    /// (0 = no vault behaviour).
    pub delay_above: u64,
    /// The wait for large transfers.
    pub delay_blocks: u64,
}

impl Policy {
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        for v in [self.daily_limit, self.per_recipient_daily, self.delay_above, self.delay_blocks] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.allow_list.len() as u64).to_le_bytes());
        for a in &self.allow_list {
            out.extend_from_slice(a);
        }
    }
}

/// What a session key may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionScope {
    /// Recipients it may pay (an app's contract, typically).
    pub recipients: Vec<AccountId>,
    /// Most per transfer.
    pub max_amount: u64,
    /// Last valid height.
    pub expires_at: u64,
}

impl SessionScope {
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.max_amount.to_le_bytes());
        out.extend_from_slice(&self.expires_at.to_le_bytes());
        out.extend_from_slice(&(self.recipients.len() as u64).to_le_bytes());
        for r in &self.recipients {
            out.extend_from_slice(r);
        }
    }

    /// Whether a transfer of `amount` to `to` at `height` is in scope.
    pub fn allows(&self, to: &AccountId, amount: u64, height: u64) -> bool {
        height <= self.expires_at && amount <= self.max_amount && self.recipients.contains(to)
    }
}
