//! Smart-account policy (ADR-135 P-5, feature bit 45): session keys and a
//! per-window spending cap, as chain-evaluated rules on account authority's
//! v9 frame (ADR-086).
//!
//! Two record kinds under the existing `ka:` layer, so a chain where nobody
//! uses them keeps its root byte for byte:
//!
//! - `ka:s:<account><key_hash>` — a session key: the policy digest that may
//!   sign plain transfers for `account`, the authority generation that granted
//!   it (digest and the height it took effect), and its scope (total cap per
//!   frame, last height, allowed recipients);
//! - `ka:w:<account>` — the window cap and what this window has moved.
//!
//! **A session key is void the moment the authority that granted it changes**
//! (recovery, or a matured `AdoptPolicy`): its row names the granting digest
//! and the height that digest took effect, and counts only while both are
//! still in force, so A->B->A does not revive it. A thief who added a session
//! key before the guardians recovered the account keeps nothing, and no sweep
//! has to find the row.
//!
//! **Bounded work.** A frame does at most three record reads (`ka:a:`,
//! `ka:s:`, `ka:w:`) and one write, a membership test over at most
//! [`MAX_SESSION_RECIPIENTS`] recipients per output, and a verification budget
//! of [`MAX_SESSION_COST`] units checked before any signature is. Nothing runs
//! at the end of a block; expired rows are never swept (their cost was paid by
//! the account that wrote them), so per-block work is the per-frame bound
//! times the frames that fit `MAX_BLOCK_BYTES` (invariant 33).

use maya_crypto_pq::suite::SuiteId;
use primitive_types::U256;

use crate::core::Transaction;
use crate::core::TxKind;
use crate::core::account_payload::{MAX_SESSION_RECIPIENTS, SessionScope};
use crate::core::codec::{ByteReader, put_amount};
use crate::core::multisig_tx::MultisigAuth;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::fees::FEE_COLLECTOR;

const SESSION_RECORD: &[u8] = b"ka:s:";
const WINDOW_RECORD: &[u8] = b"ka:w:";
const RECORD_VERSION: u8 = 1;

/// Blocks per spending window: one day at one block a second (ADR-135 O-1,
/// "Eric's answers (2026-11-12)" in ADR-135, PR #477). Windows are aligned:
/// `height - height % WINDOW_BLOCKS`.
pub const WINDOW_BLOCKS: u64 = 86_400;
/// Longest a session key may live: the one-year schedule horizon, so a
/// forgotten key cannot outlast the policy review cycle.
pub const MAX_SESSION_BLOCKS: u64 = maya_governance::limits::MAX_SCHEDULE_HORIZON_BLOCKS;
/// Verification budget of one session-signed frame, in cost units
/// (ML-DSA-65 = 1), the smart-account crate's `MAX_VALIDATION_COST`.
pub const MAX_SESSION_COST: u64 = 40;

/// Relative verification cost per suite, the table of
/// `maya_smart_account::account::verify_cost`, copied because a core crate
/// takes no new dependency. Consensus once bit 45 is on: never edit in place.
#[must_use]
pub const fn verify_cost(suite: SuiteId) -> u64 {
    match suite {
        SuiteId::Ed25519 | SuiteId::MlDsa65 => 1,
        SuiteId::MlDsa87 => 2,
        SuiteId::SlhDsaSha2_128s => 10,
        SuiteId::SlhDsaShake256f => 20,
        SuiteId::HybridMlDsa65SlhDsa128s => 11,
    }
}

/// A stored session key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    /// The authority digest in force when the key was granted.
    pub granted_by: [u8; 32],
    /// The height that authority took effect: with `granted_by`, the
    /// generation the key is bound to.
    pub granted_since: u64,
    /// What it may do.
    pub scope: SessionScope,
}

/// An account's spending window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRecord {
    /// Most moved per window; never zero in a stored row.
    pub limit: U256,
    /// First height of the window `spent` belongs to.
    pub window_start: u64,
    /// Moved so far in that window.
    pub spent: U256,
}

fn refuse<T>(why: impl Into<String>) -> Result<T> {
    Err(NodeError::Account(why.into()))
}

fn session_key(account: &Address, key_hash: &[u8; 32]) -> Vec<u8> {
    [SESSION_RECORD, account.as_slice(), key_hash.as_slice()].concat()
}

pub(crate) fn window_key(account: &Address) -> Vec<u8> {
    [WINDOW_RECORD, account.as_slice()].concat()
}

fn check_version(r: &mut ByteReader<'_>, what: &str) -> Result<()> {
    if r.read_u8()? != RECORD_VERSION {
        return Err(NodeError::Decode(format!("{what} record version")));
    }
    Ok(())
}

impl SessionRecord {
    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![RECORD_VERSION];
        out.extend_from_slice(&self.granted_by);
        out.extend_from_slice(&self.granted_since.to_le_bytes());
        self.scope.encode_into(&mut out);
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        check_version(&mut r, "session")?;
        let granted_by = r.read_array()?;
        let granted_since = r.read_u64()?;
        let scope = SessionScope::decode(&mut r)?;
        r.finish()?;
        Ok(Self {
            granted_by,
            granted_since,
            scope,
        })
    }
}

impl WindowRecord {
    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![RECORD_VERSION];
        put_amount(&mut out, &self.limit);
        out.extend_from_slice(&self.window_start.to_le_bytes());
        put_amount(&mut out, &self.spent);
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = ByteReader::new(bytes);
        check_version(&mut r, "window")?;
        let limit = r.read_amount()?;
        let window_start = r.read_u64()?;
        let spent = r.read_amount()?;
        r.finish()?;
        Ok(Self {
            limit,
            window_start,
            spent,
        })
    }

    /// What this window has moved at `height` (zero once a new one began).
    #[must_use]
    pub fn spent_at(&self, height: u64) -> U256 {
        if self.window_start == window_start(height) {
            self.spent
        } else {
            U256::zero()
        }
    }
}

/// First height of the window holding `height`.
#[must_use]
pub const fn window_start(height: u64) -> u64 {
    height - height % WINDOW_BLOCKS
}

/// Refuses a scope outside ADR-135 P-5's bounds: a non-zero cap, 1 to 16
/// distinct recipients, and a last height in `height..=height + MAX_SESSION_BLOCKS`.
fn validate(scope: &SessionScope, height: u64) -> Result<()> {
    if scope.max_amount.is_zero() {
        return refuse("a session key needs a non-zero max_amount");
    }
    let n = scope.recipients.len();
    if !(1..=MAX_SESSION_RECIPIENTS).contains(&n) {
        return refuse(format!(
            "{n} session recipients; a session key needs 1..={MAX_SESSION_RECIPIENTS}"
        ));
    }
    for (i, r) in scope.recipients.iter().enumerate() {
        if scope.recipients[..i].contains(r) {
            return refuse("a session recipient is named twice");
        }
    }
    let last = height.saturating_add(MAX_SESSION_BLOCKS);
    if !(height..=last).contains(&scope.expires_at) {
        return refuse(format!(
            "session expiry {} is outside {height}..={last}",
            scope.expires_at
        ));
    }
    Ok(())
}

impl StateDB {
    fn session_record(
        &self,
        overlay: &Overlay,
        account: &Address,
        key_hash: &[u8; 32],
    ) -> Result<Option<SessionRecord>> {
        self.record(overlay, &session_key(account, key_hash))?
            .map(|b| SessionRecord::decode(&b))
            .transpose()
    }

    fn window_record(&self, overlay: &Overlay, account: &Address) -> Result<Option<WindowRecord>> {
        self.record(overlay, &window_key(account))?
            .map(|b| WindowRecord::decode(&b))
            .transpose()
    }

    /// The committed session key `key_hash` of `account` and its window, for
    /// RPC and tests.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub fn committed_account_policy(
        &self,
        account: &Address,
        key_hash: &[u8; 32],
    ) -> Result<(Option<SessionRecord>, Option<WindowRecord>)> {
        let empty = Overlay::default();
        Ok((
            self.session_record(&empty, account, key_hash)?,
            self.window_record(&empty, account)?,
        ))
    }

    /// A v9 frame whose policy is not the account's authority: admitted only
    /// as a session-key frame inside its scope. Reads, never writes; runs
    /// before any signature is checked, at staging and in the mempool.
    ///
    /// # Errors
    ///
    /// [`NodeError::AuthorityMoved`] when no live session key matches;
    /// [`NodeError::Account`] for a frame outside the key's scope.
    pub(super) fn session_check(
        &self,
        overlay: &Overlay,
        tx: &Transaction,
        auth: &MultisigAuth,
        account: &Address,
        authority: ([u8; 32], u64),
        height: u64,
    ) -> Result<()> {
        let digest = auth.policy().digest();
        let not_signer = || {
            Err(NodeError::AuthorityMoved(format!(
                "the policy presented is neither the one signing for {} nor a live session key",
                hex::encode(account)
            )))
        };
        let Some(session) = self.session_record(overlay, account, &digest)? else {
            return not_signer();
        };
        // Granted by an authority generation no longer in force: void, even
        // if the same digest is back (A->B->A).
        if (session.granted_by, session.granted_since) != authority {
            return not_signer();
        }
        let scope = &session.scope;
        // S-1: only a plain transfer, so a session key can never sign an
        // account action (a wider scope for itself, a cancel, a new policy).
        if !matches!(tx.kind, TxKind::Transfer) {
            return refuse(format!(
                "a session key signs only plain transfers, not `{}`",
                tx.kind.label()
            ));
        }
        if height > scope.expires_at {
            return refuse(format!("the session key expired at {}", scope.expires_at));
        }
        let keys = auth.policy().keys();
        let cost = auth.approvals().iter().fold(0u64, |c, a| {
            let suite = keys.get(usize::from(a.index)).map(|k| k.suite);
            c.saturating_add(suite.map_or(u64::MAX, verify_cost))
        });
        if cost > MAX_SESSION_COST {
            return refuse(format!(
                "session approvals cost {cost} units, over the budget of {MAX_SESSION_COST}"
            ));
        }
        if let Some(o) = tx
            .outputs
            .iter()
            .find(|o| o.recipient != FEE_COLLECTOR && !scope.recipients.contains(&o.recipient))
        {
            return refuse(format!(
                "the session key may not pay {}",
                hex::encode(o.recipient)
            ));
        }
        let moved = self.vault_outflow(overlay, tx)?;
        if moved > scope.max_amount {
            return refuse(format!(
                "{moved} is over the session key's max_amount {}",
                scope.max_amount
            ));
        }
        Ok(())
    }

    /// The spending window (S-9: every frame acting for a limited account,
    /// owner-signed too). Returns the row to store once the frame applies;
    /// reads nothing below activation, where no row can exist. Counts what
    /// [`StateDB::vault_outflow`] counts: every output, the required fee exempt.
    ///
    /// A limited account sends only plain transfers and account actions, as a
    /// vault account does (ADR-030): any other kind can move value without an
    /// output (channel funding, a stake lock, an escrow, a fee debit), which
    /// the window cannot count. Fail-closed, so a kind added later is refused
    /// until someone decides how the window charges it.
    ///
    /// # Errors
    ///
    /// [`NodeError::Account`] when the frame would pass the window's limit, or
    /// is of a kind a limited account may not send.
    pub(crate) fn window_check(
        &self,
        overlay: &Overlay,
        tx: &Transaction,
        account: &Address,
        height: u64,
        activation: u64,
    ) -> Result<Option<WindowRecord>> {
        if height < activation {
            return Ok(None);
        }
        let Some(window) = self.window_record(overlay, account)? else {
            return Ok(None);
        };
        if !matches!(tx.kind, TxKind::Transfer | TxKind::Account(_)) {
            return refuse(format!(
                "an account with a daily limit sends only plain transfers and account actions, \
                 not `{}`; lift the limit first",
                tx.kind.label()
            ));
        }
        let moved = self.vault_outflow(overlay, tx)?;
        let spent = window.spent_at(height);
        let total = spent.checked_add(moved).ok_or(NodeError::BalanceOverflow)?;
        if total > window.limit {
            return refuse(format!(
                "{moved} would take this window's outflow past the account's limit {} ({spent} already used)",
                window.limit
            ));
        }
        Ok(Some(WindowRecord {
            window_start: window_start(height),
            spent: total,
            ..window
        }))
    }

    /// Stores a charged window.
    pub(crate) fn store_window(overlay: &mut Overlay, account: &Address, window: &WindowRecord) {
        StateDB::put_record(overlay, window_key(account), window.encode());
    }

    pub(super) fn add_session_key(
        &self,
        overlay: &mut Overlay,
        tx: &Transaction,
        owner: &Address,
        key_hash: &[u8; 32],
        scope: &SessionScope,
        context: BlockContext,
    ) -> Result<()> {
        validate(scope, context.height)?;
        let (authority, since) = self.opt_in(overlay, tx, owner, context.height)?;
        if &authority == key_hash {
            return refuse("the account's own authority cannot be a session key");
        }
        let record = SessionRecord {
            granted_by: authority,
            granted_since: since,
            scope: scope.clone(),
        };
        StateDB::put_record(overlay, session_key(owner, key_hash), record.encode());
        Ok(())
    }

    pub(super) fn revoke_session_key(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        key_hash: &[u8; 32],
    ) -> Result<()> {
        if self.session_record(overlay, owner, key_hash)?.is_none() {
            return refuse("no such session key");
        }
        StateDB::delete_record(overlay, session_key(owner, key_hash));
        Ok(())
    }

    pub(super) fn set_daily_limit(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        limit: U256,
        height: u64,
    ) -> Result<()> {
        let k = window_key(owner);
        if limit.is_zero() {
            StateDB::delete_record(overlay, k);
            return Ok(());
        }
        // The window already charged keeps counting under the new limit.
        let spent = self
            .window_record(overlay, owner)?
            .map_or_else(U256::zero, |w| w.spent_at(height));
        let window = WindowRecord {
            limit,
            window_start: window_start(height),
            spent,
        };
        StateDB::put_record(overlay, k, window.encode());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn scope(n: usize, expires_at: u64) -> SessionScope {
        SessionScope {
            max_amount: U256::from(100u64),
            expires_at,
            recipients: (0..n).map(|i| [u8::try_from(i + 1).unwrap(); 32]).collect(),
        }
    }

    #[test]
    fn records_round_trip_and_windows_roll_over() {
        let s = SessionRecord {
            granted_by: [3; 32],
            granted_since: 7,
            scope: scope(2, 50),
        };
        assert_eq!(SessionRecord::decode(&s.encode()).unwrap(), s);
        let w = WindowRecord {
            limit: U256::from(10u64),
            window_start: WINDOW_BLOCKS,
            spent: U256::from(4u64),
        };
        assert_eq!(WindowRecord::decode(&w.encode()).unwrap(), w);
        assert_eq!(w.spent_at(2 * WINDOW_BLOCKS - 1), U256::from(4u64));
        assert_eq!(w.spent_at(2 * WINDOW_BLOCKS), U256::zero());
    }

    #[test]
    fn scope_bounds() {
        assert!(validate(&scope(1, 10), 10).is_ok());
        assert!(validate(&scope(MAX_SESSION_RECIPIENTS, 10 + MAX_SESSION_BLOCKS), 10).is_ok());
        assert!(validate(&scope(0, 10), 10).is_err());
        assert!(validate(&scope(1, 9), 10).is_err(), "already expired");
        assert!(validate(&scope(1, 11 + MAX_SESSION_BLOCKS), 10).is_err());
        let mut zero = scope(1, 10);
        zero.max_amount = U256::zero();
        assert!(validate(&zero, 10).is_err());
        let mut twice = scope(2, 10);
        twice.recipients[1] = twice.recipients[0];
        assert!(validate(&twice, 10).is_err());
    }
}
