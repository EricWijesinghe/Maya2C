//! Vault accounts (ADR-030): delayed withdrawals a guardian can cancel.
//!
//! Two records under `q:`, a layer of the state root:
//!
//! - `q:v:<owner>` — the policy, a queued replacement and when it matures, the
//!   next request id and how many requests are open;
//! - `q:w:<owner><id>` — one withdrawal: recipient, amount, maturity, status.
//!
//! A queued policy takes effect by height, not by transaction: whoever reads
//! the record at or after its maturity sees the new policy, so nothing has to
//! be sent for it to apply. Every action writes the record back normalised.

use maya_ledger_math as ledger_math;

use crate::core::Transaction;
use crate::core::TxKind;
use crate::core::codec::ByteReader;
use crate::core::vault_payload::{MAX_GUARDIANS, RECONFIGURE_ID, VaultAction, VaultConfig};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

/// The vault layer's prefix.
pub(crate) const VAULT_PREFIX: &[u8] = b"q:";
/// Prefix of withdrawal records, for the conservation guard.
pub(crate) const WITHDRAWAL_PREFIX: &[u8] = b"q:w:";
const RECORD_PREFIX: &[u8] = b"q:v:";

/// Longest delay a vault may choose.
pub const MAX_DELAY_BLOCKS: u64 = 1_000_000;
/// Most requests open at once in one vault.
pub const MAX_OPEN_REQUESTS: u32 = 64;
/// Multiple of the required fee a vault transaction may pay before the excess
/// counts against its limit. Wallets pay 2x (`l1-wallet`), so 4x is headroom
/// for a base fee that rose between signing and inclusion.
pub const VAULT_FEE_HEADROOM: u128 = 4;

const OPEN: u8 = 0;
const EXECUTED: u8 = 1;
const CANCELLED: u8 = 2;

/// A vault's record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultRecord {
    /// The policy in force (before any queued replacement matures).
    pub config: VaultConfig,
    /// A replacement and the height it takes effect.
    pub pending: Option<(VaultConfig, u64)>,
    /// The next request's id.
    pub next_id: u64,
    /// Requests neither executed nor cancelled.
    pub open: u32,
    /// Start of the current instant-outflow window.
    pub window_start: u64,
    /// Instant outflow so far in that window.
    pub window_spent: u64,
}

/// One withdrawal request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Withdrawal {
    /// Recipient.
    pub to: Address,
    /// Escrowed amount.
    pub amount: u64,
    /// First height it may execute.
    pub ready_height: u64,
    /// 0 open, 1 executed, 2 cancelled.
    pub status: u8,
}

impl VaultRecord {
    /// The policy at `height`: the queued one once it has matured.
    #[must_use]
    pub fn effective(&self, height: u64) -> &VaultConfig {
        match &self.pending {
            Some((next, ready)) if *ready <= height => next,
            _ => &self.config,
        }
    }

    fn normalised(mut self, height: u64) -> Self {
        if let Some((next, ready)) = self.pending.take() {
            if ready <= height {
                self.config = next;
            } else {
                self.pending = Some((next, ready));
            }
        }
        self
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = vec![1u8];
        self.config.encode_into(&mut out);
        match &self.pending {
            Some((next, ready)) => {
                out.push(1);
                next.encode_into(&mut out);
                out.extend_from_slice(&ready.to_le_bytes());
            }
            None => out.push(0),
        }
        out.extend_from_slice(&self.next_id.to_le_bytes());
        out.extend_from_slice(&self.open.to_le_bytes());
        out.extend_from_slice(&self.window_start.to_le_bytes());
        out.extend_from_slice(&self.window_spent.to_le_bytes());
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != 1 {
            return Err(NodeError::Decode("vault record version".into()));
        }
        let config = VaultConfig::decode(&mut r)?;
        let pending = match r.read_u8()? {
            0 => None,
            1 => Some((VaultConfig::decode(&mut r)?, r.read_u64()?)),
            t => return Err(NodeError::Decode(format!("vault pending tag {t}"))),
        };
        let next_id = r.read_u64()?;
        let open = u32::from_le_bytes(r.read_array()?);
        let window_start = r.read_u64()?;
        let window_spent = r.read_u64()?;
        r.finish()?;
        Ok(Self {
            config,
            pending,
            next_id,
            open,
            window_start,
            window_spent,
        })
    }
}

impl Withdrawal {
    /// What the request holds in escrow: its amount while open.
    #[must_use]
    pub fn escrowed(&self) -> u64 {
        if self.status == OPEN { self.amount } else { 0 }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = vec![1u8];
        out.extend_from_slice(&self.to);
        out.extend_from_slice(&self.amount.to_le_bytes());
        out.extend_from_slice(&self.ready_height.to_le_bytes());
        out.push(self.status);
        out
    }

    /// Decodes a withdrawal.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a wrong version or malformed bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        if r.read_u8()? != 1 {
            return Err(NodeError::Decode("withdrawal version".into()));
        }
        let w = Self {
            to: r.read_array()?,
            amount: r.read_u64()?,
            ready_height: r.read_u64()?,
            status: r.read_u8()?,
        };
        r.finish()?;
        Ok(w)
    }
}

fn record_key(owner: &Address) -> Vec<u8> {
    [RECORD_PREFIX, owner.as_slice()].concat()
}

fn withdrawal_key(owner: &Address, id: u64) -> Vec<u8> {
    [WITHDRAWAL_PREFIX, owner.as_slice(), &id.to_be_bytes()].concat()
}

fn refuse<T>(why: impl Into<String>) -> Result<T> {
    Err(NodeError::Vault(why.into()))
}

fn validate(config: &VaultConfig) -> Result<()> {
    if !(1..=MAX_DELAY_BLOCKS).contains(&config.delay_blocks) {
        return refuse(format!(
            "delay of {} blocks is outside 1..={MAX_DELAY_BLOCKS}",
            config.delay_blocks
        ));
    }
    if config.guardians.is_empty() || config.guardians.len() > MAX_GUARDIANS {
        return refuse(format!(
            "{} guardians; a vault needs 1..={MAX_GUARDIANS}",
            config.guardians.len()
        ));
    }
    Ok(())
}

impl StateDB {
    fn vault_record(&self, overlay: &Overlay, owner: &Address) -> Result<Option<VaultRecord>> {
        self.record(overlay, &record_key(owner))?
            .map(|b| VaultRecord::decode(&b))
            .transpose()
    }

    /// The committed vault of `owner`, for RPC.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub fn committed_vault(&self, owner: &Address) -> Result<Option<VaultRecord>> {
        self.raw_get(&record_key(owner))?
            .map(|b| VaultRecord::decode(&b))
            .transpose()
    }

    /// The committed withdrawals of `owner`, by id.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub fn committed_withdrawals(&self, owner: &Address) -> Result<Vec<(u64, Withdrawal)>> {
        let prefix = [WITHDRAWAL_PREFIX, owner.as_slice()].concat();
        self.scan_prefix(&prefix)?
            .into_iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| {
                let id = k[prefix.len()..]
                    .try_into()
                    .map(u64::from_be_bytes)
                    .map_err(|_| NodeError::Decode("withdrawal key".into()))?;
                Ok((id, Withdrawal::decode(&v)?))
            })
            .collect()
    }

    /// Refuses what a vault account may not send (ADR-030) and returns the
    /// vault record with this transaction's instant outflow charged, for the
    /// caller to store; `None` for an ordinary account, which passes
    /// untouched.
    ///
    /// Allowed: plain transfers and vault actions, whose outputs together stay
    /// within `limit` **per window** of `delay_blocks`. A per-transaction limit
    /// let a stolen key drain a vault through thousands of in-limit transfers
    /// in one round (review). Fee outputs count too, above a headroom over what
    /// the fee rule requires: excluding them outright let a stolen key send the
    /// whole balance to the collector at once (review, C1).
    pub(crate) fn vault_check(
        &self,
        overlay: &Overlay,
        tx: &Transaction,
        sender: &Address,
        height: u64,
    ) -> Result<Option<VaultRecord>> {
        let Some(vault) = self.vault_record(overlay, sender)? else {
            return Ok(None);
        };
        if !matches!(tx.kind, TxKind::Transfer | TxKind::Vault(_)) {
            return refuse(format!(
                "a vault account may not send `{}`",
                tx.kind.label()
            ));
        }
        let moved = self.vault_outflow(overlay, tx)?;
        let mut vault = vault.normalised(height);
        let config = vault.config.clone();
        if height >= vault.window_start.saturating_add(config.delay_blocks) {
            vault.window_start = height;
            vault.window_spent = 0;
        }
        let spent = u128::from(vault.window_spent).saturating_add(moved);
        if spent > u128::from(config.limit) {
            return refuse(format!(
                "{moved} would take this window's outflow past the vault's instant limit {}                  ({} already used); request a delayed withdrawal",
                config.limit, vault.window_spent
            ));
        }
        vault.window_spent = u64::try_from(spent).map_err(|_| NodeError::BalanceOverflow)?;
        Ok(Some(vault))
    }

    /// Value `tx` moves out: every non-fee output, plus fee outputs above
    /// [`VAULT_FEE_HEADROOM`] times the required fee.
    fn vault_outflow(&self, overlay: &Overlay, tx: &Transaction) -> Result<u128> {
        let collector = crate::state::fees::FEE_COLLECTOR;
        let sum = |fee: bool| -> u128 {
            tx.outputs
                .iter()
                .filter(|o| (o.recipient == collector) == fee)
                .map(|o| u128::from(o.amount))
                .sum()
        };
        let allowed_fee = match self.fee_record(overlay)? {
            Some(fees) => fees
                .required(tx.to_bytes().len())
                .saturating_mul(VAULT_FEE_HEADROOM),
            None => 0,
        };
        Ok(sum(false).saturating_add(sum(true).saturating_sub(allowed_fee)))
    }

    /// Stores `vault` as `owner`'s record: how a charged window is kept.
    pub(crate) fn store_vault(overlay: &mut Overlay, owner: &Address, vault: &VaultRecord) {
        StateDB::put_record(overlay, record_key(owner), vault.encode());
    }

    /// Executes a vault action sent by `sender`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Vault`] for a refused action; balance and storage errors.
    pub(crate) fn apply_vault(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        action: &VaultAction,
        context: BlockContext,
    ) -> Result<()> {
        match action {
            VaultAction::Configure(config) => {
                self.vault_configure(overlay, sender, config, context.height)
            }
            VaultAction::Request { to, amount } => {
                self.vault_request(overlay, sender, to, *amount, context.height)
            }
            VaultAction::Execute { owner, id } => {
                self.vault_execute(overlay, owner, *id, context.height)
            }
            VaultAction::Cancel { owner, id } => {
                self.vault_cancel(overlay, sender, owner, *id, context.height)
            }
        }
    }

    fn vault_configure(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        config: &VaultConfig,
        height: u64,
    ) -> Result<()> {
        validate(config)?;
        let record = match self.vault_record(overlay, owner)? {
            None => VaultRecord {
                config: config.clone(),
                pending: None,
                next_id: 0,
                open: 0,
                window_start: height,
                window_spent: 0,
            },
            Some(existing) => {
                // Queued behind the *current* delay: a thief with the key
                // cannot shorten the wait before the wait has passed.
                let existing = existing.normalised(height);
                let ready = height.saturating_add(existing.config.delay_blocks);
                VaultRecord {
                    pending: Some((config.clone(), ready)),
                    ..existing
                }
            }
        };
        StateDB::put_record(overlay, record_key(owner), record.encode());
        Ok(())
    }

    fn vault_request(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        to: &Address,
        amount: u64,
        height: u64,
    ) -> Result<()> {
        let Some(vault) = self.vault_record(overlay, owner)? else {
            return refuse("the sender has no vault");
        };
        let mut vault = vault.normalised(height);
        if amount == 0 {
            return refuse("a withdrawal must move something");
        }
        if vault.open >= MAX_OPEN_REQUESTS {
            return refuse(format!("{MAX_OPEN_REQUESTS} requests are already open"));
        }
        let mut account = self.load(overlay, owner)?;
        let available = account.balance;
        account.balance = ledger_math::debit(available, amount).ok_or_else(|| {
            NodeError::InsufficientBalance {
                address: hex::encode(owner),
                required: amount,
                available,
            }
        })?;
        overlay.accounts.insert(*owner, account);
        let id = vault.next_id;
        let ready_height = height.saturating_add(vault.config.delay_blocks);
        StateDB::put_record(
            overlay,
            withdrawal_key(owner, id),
            Withdrawal {
                to: *to,
                amount,
                ready_height,
                status: OPEN,
            }
            .encode(),
        );
        vault.next_id = vault
            .next_id
            .checked_add(1)
            .ok_or(NodeError::BalanceOverflow)?;
        vault.open += 1;
        StateDB::put_record(overlay, record_key(owner), vault.encode());
        Ok(())
    }

    /// Settles an open request: pays `to` (execute) or `owner` (cancel).
    fn vault_settle(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        id: u64,
        status: u8,
        height: u64,
    ) -> Result<()> {
        let key = withdrawal_key(owner, id);
        let Some(bytes) = self.record(overlay, &key)? else {
            return refuse(format!(
                "no open request {id}: never made, or already settled"
            ));
        };
        let w = Withdrawal::decode(&bytes)?;
        if w.status != OPEN {
            return refuse(format!("request {id} is already settled"));
        }
        if status == EXECUTED && height < w.ready_height {
            return refuse(format!(
                "request {id} matures at {}, not {height}",
                w.ready_height
            ));
        }
        let payee = if status == EXECUTED { w.to } else { *owner };
        let mut account = self.load(overlay, &payee)?;
        account.balance =
            ledger_math::credit(account.balance, w.amount).ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(payee, account);
        // Settled requests are deleted, not kept with a status: history is in
        // the blocks, and a vault's record set stays bounded by its open
        // requests (review). A deleted request cannot be settled twice.
        overlay.records.insert(key, None);
        let vault = self
            .vault_record(overlay, owner)?
            .ok_or_else(|| NodeError::Vault("request without a vault".into()))?;
        let open = vault.open.checked_sub(1).ok_or_else(|| {
            NodeError::InvariantViolation(format!(
                "vault {} settles with no open request",
                hex::encode(owner)
            ))
        })?;
        let vault = VaultRecord {
            open,
            ..vault.normalised(height)
        };
        StateDB::put_record(overlay, record_key(owner), vault.encode());
        Ok(())
    }

    fn vault_execute(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        id: u64,
        height: u64,
    ) -> Result<()> {
        self.vault_settle(overlay, owner, id, EXECUTED, height)
    }

    fn vault_cancel(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        owner: &Address,
        id: u64,
        height: u64,
    ) -> Result<()> {
        let Some(vault) = self.vault_record(overlay, owner)? else {
            return refuse("no such vault");
        };
        let vault = vault.normalised(height);
        if sender != owner && !vault.config.guardians.contains(sender) {
            return refuse("only a guardian or the owner may cancel");
        }
        if id != RECONFIGURE_ID {
            return self.vault_settle(overlay, owner, id, CANCELLED, height);
        }
        if vault.pending.is_none() {
            return refuse("no reconfiguration is queued");
        }
        StateDB::put_record(
            overlay,
            record_key(owner),
            VaultRecord {
                pending: None,
                ..vault
            }
            .encode(),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn config(delay: u64) -> VaultConfig {
        VaultConfig {
            delay_blocks: delay,
            limit: 100,
            guardians: vec![[7; 32]],
        }
    }

    #[test]
    fn records_round_trip_and_a_queued_policy_matures_by_height() {
        let r = VaultRecord {
            config: config(10),
            pending: Some((config(3), 50)),
            next_id: 4,
            open: 2,
            window_start: 30,
            window_spent: 70,
        };
        assert_eq!(VaultRecord::decode(&r.encode()).unwrap(), r);
        assert_eq!(r.effective(49).delay_blocks, 10);
        assert_eq!(r.effective(50).delay_blocks, 3);
        let w = Withdrawal {
            to: [1; 32],
            amount: 9,
            ready_height: 20,
            status: OPEN,
        };
        assert_eq!(Withdrawal::decode(&w.encode()).unwrap(), w);
        assert_eq!(w.escrowed(), 9);
        assert_eq!(
            Withdrawal {
                status: CANCELLED,
                ..w
            }
            .escrowed(),
            0
        );
    }
}
