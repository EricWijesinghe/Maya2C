//! Value conservation: the one equation every block has to satisfy.
//!
//! ## The equation
//!
//! For every asset, native and registered, what a block moves must equal what
//! it issues:
//!
//! ```text
//! Σ holdings delta  ==  supply delta
//! ```
//!
//! The left side is every place on this chain that can hold value, which is a
//! closed list:
//!
//! | Asset | Held in |
//! |---|---|
//! | Native | `acct:` balances, `chan:` capacity, the shielded pool's public balance, `g:lock:` stakes, `g:prop:` deposits, `h:lk:` HTLC escrow, `k:state` validator stake |
//! | Registered | `d:bal:` balances, `d:pool:` reserves, `d:ord:` escrow |
//! | LP share | `d:bal:` balances |
//!
//! The right side is zero for the native coin — there is no mint operation and
//! no block reward, so the genesis allocation is the supply forever. For a
//! registered asset it is the change in `d:asset:<id>.total_supply`, which is
//! non-zero exactly once, at registration. For a share asset it is the change
//! in its pool's `total_shares`.
//!
//! ## Why a delta and not a sum
//!
//! Summing every account each block would make block execution O(state), so
//! execution would slow down for the life of the chain. The overlay already
//! holds exactly what the block touched, so every term here is a difference
//! between an overlay value and its committed one: O(touched), which for a
//! normal block is a handful of keys.
//!
//! It is also not weaker. Conservation over deltas, applied to every block from
//! genesis, is an induction: if the total was right before the block and the
//! block moved a net zero, the total is right after it. An absolute
//! `MAX_SUPPLY` scan would check the same fact more slowly, and only for the
//! assets it remembered to look at.
//!
//! ## Why a violation fails the block
//!
//! This is not an anomaly threshold, and there is no judgement in it. A block
//! that creates a unit from nothing is invalid in the same way a block with the
//! wrong state root is invalid — it is refused, nothing is committed, and no
//! breaker is written, because there is no state to protect. The
//! [breaker](super::breaker) exists for the other case: a block that is valid
//! and suspicious.

use std::collections::BTreeMap;

use maya_dex::amm::MINIMUM_LIQUIDITY;
use maya_dex::types::NATIVE_ASSET;

use crate::error::{NodeError, Result};
use crate::governance::{LOCK_PREFIX, LockRecord, PROPOSAL_PREFIX, ProposalRecord};
use crate::state::asset::{AssetId, AssetRecord, decode_balance};
use crate::state::channel::{ChannelRecord, ChannelStatus};
use crate::state::db::{Overlay, StateDB};
use crate::state::dex::{
    ASSET_PREFIX, BALANCE_PREFIX, LP_ASSET_PREFIX, ORDER_PREFIX, OrderRecord, POOL_PREFIX,
    PoolRecord, derive_lp_asset, pool_key,
};
use crate::state::htlc::{LOCK_PREFIX as HTLC_LOCK_PREFIX, decode as htlc_decode};

/// A signed running total per asset.
///
/// Holdings add, issuance subtracts, and every entry must end at zero. One map
/// rather than two compared afterwards: an asset that appears on only one side
/// is then a non-zero entry rather than a lookup somebody has to remember to
/// do in both directions.
type Ledger = BTreeMap<AssetId, i128>;

fn shift(ledger: &mut Ledger, asset: AssetId, delta: i128) {
    if delta != 0 {
        *ledger.entry(asset).or_insert(0) += delta;
    }
}

/// `new - old`, as a signed difference that cannot overflow.
fn diff(new: u64, old: u64) -> i128 {
    i128::from(new) - i128::from(old)
}

/// The value a channel is still holding: nothing, once it has paid out.
fn escrowed(record: &ChannelRecord) -> u64 {
    match record.status {
        ChannelStatus::Closed => 0,
        ChannelStatus::Open | ChannelStatus::Disputed => record.capacity,
    }
}

/// The 32-byte identifier a key carries after `prefix`.
///
/// # Errors
///
/// Returns [`NodeError::Decode`] if the key does not carry exactly one
/// identifier and nothing more.
///
/// Deliberately *not* `Option` with the caller skipping a `None`. This module
/// exists to make a bug elsewhere visible as an imbalance rather than as
/// silence, and a key of the wrong shape under a prefix that holds value is
/// exactly such a bug — skipping it would drop the record from the ledger and
/// let the block balance. Every key this chain writes is built by a fixed
/// helper, so this is unreachable today and deterministic if it ever is not:
/// every node constructs the same key and so every node refuses the same
/// block.
fn id_after(key: &[u8], prefix: &[u8]) -> Result<[u8; 32]> {
    let malformed = || {
        NodeError::Decode(format!(
            "conservation: key {} is not a 32-byte id under {}",
            hex::encode(key),
            String::from_utf8_lossy(prefix)
        ))
    };
    key.strip_prefix(prefix)
        .ok_or_else(malformed)?
        .try_into()
        .map_err(|_| malformed())
}

impl StateDB {
    /// Checks that `overlay` neither creates nor destroys value.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvariantViolation`] naming the asset and the
    /// amount it is out by, or a read or decode failure from committed state.
    pub(crate) fn check_value_conservation(&self, overlay: &Overlay) -> Result<()> {
        let mut ledger = Ledger::new();

        self.fold_native(overlay, &mut ledger)?;
        self.fold_records(overlay, &mut ledger)?;

        match ledger.iter().find(|(_, net)| **net != 0) {
            Some((asset, net)) => Err(NodeError::InvariantViolation(format!(
                "asset {} is out by {net} units: holdings moved by that much more than supply",
                hex::encode(asset)
            ))),
            None => Ok(()),
        }
    }

    /// The four places the native coin lives outside the trading keyspace.
    fn fold_native(&self, overlay: &Overlay, ledger: &mut Ledger) -> Result<()> {
        for (address, account) in &overlay.accounts {
            let committed = self.get_account(address)?;
            shift(
                ledger,
                NATIVE_ASSET,
                diff(account.balance, committed.balance),
            );
        }

        for (id, record) in &overlay.channels {
            let previous = match self.get_channel(id)? {
                Some(committed) => escrowed(&committed),
                None => 0,
            };
            shift(ledger, NATIVE_ASSET, diff(escrowed(record), previous));
        }

        if let Some(pool) = &overlay.shielded {
            let committed = self.stored_pool()?;
            shift(
                ledger,
                NATIVE_ASSET,
                diff(pool.balance(), committed.balance()),
            );
        }

        Ok(())
    }

    /// Every generic record the block wrote that holds or issues value.
    ///
    /// Driven by the overlay rather than by a scan, and by prefix rather than
    /// by subsystem, so a record kind that holds value and is not listed here
    /// shows up as a non-zero entry somewhere else in the ledger rather than as
    /// silence.
    fn fold_records(&self, overlay: &Overlay, ledger: &mut Ledger) -> Result<()> {
        for (key, staged) in &overlay.records {
            let previous = self.raw_get(key)?;
            let previous = previous.as_deref();
            let staged = staged.as_deref();

            if key.starts_with(BALANCE_PREFIX) {
                self.fold_balance(key, staged, previous, ledger)?;
            } else if key.starts_with(POOL_PREFIX) {
                fold_pool(key, staged, previous, ledger)?;
            } else if key.starts_with(ORDER_PREFIX) {
                self.fold_order(overlay, staged, previous, ledger)?;
            } else if key.starts_with(ASSET_PREFIX) {
                fold_asset(key, staged, previous, ledger)?;
            } else if key.starts_with(LOCK_PREFIX) {
                fold_lock(staged, previous, ledger)?;
            } else if key.starts_with(PROPOSAL_PREFIX) {
                fold_proposal(staged, previous, ledger)?;
            } else if key.starts_with(HTLC_LOCK_PREFIX) {
                fold_htlc_lock(staged, previous, ledger)?;
            } else if key.as_slice() == crate::state::staking::STATE_KEY {
                fold_staking(staged, previous, ledger)?;
            }
        }
        Ok(())
    }

    /// `d:bal:<asset><holder>`: one holder's balance in one asset.
    fn fold_balance(
        &self,
        key: &[u8],
        staged: Option<&[u8]>,
        previous: Option<&[u8]>,
        ledger: &mut Ledger,
    ) -> Result<()> {
        // The asset is the first 32 bytes after the prefix; the holder is the
        // rest. A tail too short to carry one is a malformed key, and silence
        // here would drop a balance change from the ledger and let a block
        // that moved value balance — see [`id_after`].
        let tail = key.strip_prefix(BALANCE_PREFIX).unwrap_or_default();
        let asset: [u8; 32] = tail
            .get(..32)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| {
                NodeError::Decode(format!(
                    "conservation: balance key {} carries no asset id",
                    hex::encode(key)
                ))
            })?;
        shift(ledger, asset, diff(balance(staged)?, balance(previous)?));
        Ok(())
    }

    /// `d:ord:<pair><side><sort key>`: one resting order's escrow.
    ///
    /// Which asset it escrows depends on the side and the pool's assets, and a
    /// pool's assets are fixed at creation, so reading them through the overlay
    /// and reading them from committed state give the same answer.
    fn fold_order(
        &self,
        overlay: &Overlay,
        staged: Option<&[u8]>,
        previous: Option<&[u8]>,
        ledger: &mut Ledger,
    ) -> Result<()> {
        for (bytes, sign) in [(staged, 1i128), (previous, -1i128)] {
            let Some(bytes) = bytes else { continue };
            let order = OrderRecord::decode(bytes)?;
            let Some(pool) = self.record(overlay, &pool_key(&order.pair))? else {
                // An order whose pool does not exist cannot have been written
                // by this chain's rules. Leaving its escrow uncounted would
                // make it a hole in the equation, so say so instead.
                return Err(NodeError::InvariantViolation(format!(
                    "order on pair {} has no pool",
                    hex::encode(order.pair)
                )));
            };
            let pool = PoolRecord::decode(&pool)?;
            shift(
                ledger,
                order.escrow_asset(&pool),
                sign * i128::from(order.escrow),
            );
        }
        Ok(())
    }
}

/// `d:pool:<pair>`: both reserves, and the share asset the pool issues.
fn fold_pool(
    key: &[u8],
    staged: Option<&[u8]>,
    previous: Option<&[u8]>,
    ledger: &mut Ledger,
) -> Result<()> {
    let pair = id_after(key, POOL_PREFIX)?;
    let staged = staged.map(PoolRecord::decode).transpose()?;
    let previous = previous.map(PoolRecord::decode).transpose()?;

    let reserves = |record: Option<&PoolRecord>| {
        record.map_or((0, 0, 0), |record| {
            (
                record.pool.reserve_base,
                record.pool.reserve_quote,
                holdable_shares(record.pool.total_shares),
            )
        })
    };
    let (new_base, new_quote, new_shares) = reserves(staged.as_ref());
    let (old_base, old_quote, old_shares) = reserves(previous.as_ref());

    // A pool's assets are fixed at creation, so either record names them and
    // the two agree. A block that created the pool has only the staged one.
    let assets = staged
        .as_ref()
        .or(previous.as_ref())
        .map(|record| (record.base_asset, record.quote_asset));
    let Some((base_asset, quote_asset)) = assets else {
        return Ok(());
    };

    shift(ledger, base_asset, diff(new_base, old_base));
    shift(ledger, quote_asset, diff(new_quote, old_quote));
    // Shares are the pool's own issuance, so they enter with the opposite sign
    // to the balances that hold them.
    shift(
        ledger,
        derive_lp_asset(&pair),
        -diff(new_shares, old_shares),
    );
    Ok(())
}

/// The share supply somebody can actually hold.
///
/// [`MINIMUM_LIQUIDITY`] shares are minted to nobody on a pool's first deposit,
/// deliberately: without them the first depositor can donate into the reserves
/// and make one share worth so much that every later deposit rounds down to
/// zero. So the pool's `total_shares` is permanently a thousand more than the
/// sum of every balance in its share asset, and an equation that did not
/// subtract them would refuse the creation of every pool on this chain.
///
/// Saturating rather than checked because a pool that exists has at least this
/// many shares — `Pool::seed` refuses a first deposit that cannot cover them —
/// and a pool that does not exist has zero, which is the answer either way.
fn holdable_shares(total: u64) -> u64 {
    total.saturating_sub(MINIMUM_LIQUIDITY)
}

/// `d:asset:<id>`: a registered asset's fixed supply, issued once.
fn fold_asset(
    key: &[u8],
    staged: Option<&[u8]>,
    previous: Option<&[u8]>,
    ledger: &mut Ledger,
) -> Result<()> {
    let asset = id_after(key, ASSET_PREFIX)?;
    let supply = |bytes: Option<&[u8]>| -> Result<u64> {
        Ok(match bytes {
            Some(bytes) => AssetRecord::decode(bytes)?.total_supply,
            None => 0,
        })
    };
    shift(ledger, asset, -diff(supply(staged)?, supply(previous)?));
    Ok(())
}

/// `g:lock:<address>`: native coin held as voting weight.
fn fold_lock(staged: Option<&[u8]>, previous: Option<&[u8]>, ledger: &mut Ledger) -> Result<()> {
    let amount = |bytes: Option<&[u8]>| -> Result<u64> {
        Ok(match bytes {
            Some(bytes) => LockRecord::decode(bytes)?.amount,
            None => 0,
        })
    };
    shift(
        ledger,
        NATIVE_ASSET,
        diff(amount(staged)?, amount(previous)?),
    );
    Ok(())
}

/// `g:prop:<id>`: a proposal's anti-spam deposit, held until it settles.
fn fold_proposal(
    staged: Option<&[u8]>,
    previous: Option<&[u8]>,
    ledger: &mut Ledger,
) -> Result<()> {
    let deposit = |bytes: Option<&[u8]>| -> Result<u64> {
        Ok(match bytes {
            Some(bytes) => ProposalRecord::decode(bytes)?.deposit,
            None => 0,
        })
    };
    shift(
        ledger,
        NATIVE_ASSET,
        diff(deposit(staged)?, deposit(previous)?),
    );
    Ok(())
}

/// `h:lk:<id>`: native coin escrowed by a lattice HTLC until it settles.
fn fold_htlc_lock(
    staged: Option<&[u8]>,
    previous: Option<&[u8]>,
    ledger: &mut Ledger,
) -> Result<()> {
    let escrowed = |bytes: Option<&[u8]>| -> Result<u64> {
        Ok(match bytes {
            Some(bytes) => htlc_decode(bytes)?.escrowed(),
            None => 0,
        })
    };
    shift(
        ledger,
        NATIVE_ASSET,
        diff(escrowed(staged)?, escrowed(previous)?),
    );
    Ok(())
}

/// `k:state`: native coin held by staking — bonds, delegations and funds
/// waiting out the unbonding delay (ADR-028). One record, so the delta is
/// the difference of two totals.
fn fold_staking(staged: Option<&[u8]>, previous: Option<&[u8]>, ledger: &mut Ledger) -> Result<()> {
    let held = |bytes: Option<&[u8]>| -> Result<i128> {
        Ok(match bytes {
            Some(bytes) => i128::try_from(
                crate::state::staking::StakingRecord::decode(bytes)?
                    .staking
                    .held(),
            )
            .map_err(|_| NodeError::InvariantViolation("staking holds more than i128".into()))?,
            None => 0,
        })
    };
    shift(ledger, NATIVE_ASSET, held(staged)? - held(previous)?);
    Ok(())
}

/// An absent balance record is a zero balance, not a missing one.
fn balance(bytes: Option<&[u8]>) -> Result<u64> {
    match bytes {
        Some(bytes) => decode_balance(bytes),
        None => Ok(0),
    }
}

/// The share-asset marker prefix is named here only so that a reader who goes
/// looking for it finds the reason it is absent above: `d:lp:<asset> -> pair`
/// records which pool issues a share asset, and holds no value itself. The
/// issuance it stands for is counted from the pool's `total_shares`.
const _: &[u8] = LP_ASSET_PREFIX;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::state::account::Account;
    use crate::state::asset::encode_balance;
    use crate::state::dex::{asset_key, balance_key};
    use crate::state::shielded::ShieldedPool;
    use maya_dex::FeeSchedule;
    use maya_dex::amm::Pool;
    use tempfile::TempDir;

    const PAIR: [u8; 32] = [3u8; 32];
    const QUOTE: [u8; 32] = [7u8; 32];
    const ALICE: [u8; 32] = [1u8; 32];
    const BOB: [u8; 32] = [2u8; 32];

    fn open() -> (StateDB, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        (StateDB::open(dir.path()).expect("open"), dir)
    }

    fn account(balance: u64) -> Account {
        Account { balance, nonce: 0 }
    }

    fn pool_record(base: u64, quote: u64, shares: u64) -> PoolRecord {
        PoolRecord {
            base_asset: NATIVE_ASSET,
            quote_asset: QUOTE,
            pool: Pool {
                reserve_base: base,
                reserve_quote: quote,
                total_shares: shares,
                fees: FeeSchedule {
                    lp_bps: 30,
                    protocol_bps: 0,
                },
            },
        }
    }

    /// The violation's message, so a test can say which asset was wrong and by
    /// how much rather than only that something was.
    fn violation(db: &StateDB, overlay: &Overlay) -> String {
        match db.check_value_conservation(overlay) {
            Err(NodeError::InvariantViolation(reason)) => reason,
            other => panic!("expected a conservation violation, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_block_conserves_everything() {
        let (db, _dir) = open();
        db.check_value_conservation(&Overlay::default())
            .expect("nothing moved");
    }

    #[test]
    fn moving_native_coin_between_two_accounts_conserves_it() {
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");

        let mut overlay = Overlay::default();
        overlay.accounts.insert(ALICE, account(400));
        overlay.accounts.insert(BOB, account(600));
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn crediting_an_account_from_nowhere_is_refused() {
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");

        let mut overlay = Overlay::default();
        overlay.accounts.insert(ALICE, account(1_001));
        assert!(violation(&db, &overlay).contains("out by 1 units"));
    }

    #[test]
    fn burning_native_coin_is_refused_exactly_as_minting_it_is() {
        // Destroying value is checked for the same reason creating it is: a
        // subsystem that loses a unit on a rounding path loses it forever, and
        // nothing else on this chain would ever notice.
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");

        let mut overlay = Overlay::default();
        overlay.accounts.insert(ALICE, account(999));
        assert!(violation(&db, &overlay).contains("out by -1 units"));
    }

    #[test]
    fn escrowing_into_a_channel_conserves_the_coin_it_holds() {
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");

        let mut overlay = Overlay::default();
        overlay.accounts.insert(ALICE, account(400));
        overlay
            .channels
            .insert([9u8; 32], ChannelRecord::new(ALICE, BOB, 600, 10));
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn a_channel_that_escrows_more_than_it_was_funded_is_refused() {
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");

        let mut overlay = Overlay::default();
        overlay.accounts.insert(ALICE, account(400));
        overlay
            .channels
            .insert([9u8; 32], ChannelRecord::new(ALICE, BOB, 900, 10));
        assert!(violation(&db, &overlay).contains("out by 300"));
    }

    #[test]
    fn a_closed_channel_pays_out_exactly_what_it_escrowed() {
        let (db, _dir) = open();
        let mut open_channel = ChannelRecord::new(ALICE, BOB, 600, 10);
        db.put_channel(&[9u8; 32], &open_channel).expect("escrow");
        db.put_account(&ALICE, &account(0)).expect("fund");

        open_channel.status = ChannelStatus::Closed;
        let mut overlay = Overlay::default();
        overlay.channels.insert([9u8; 32], open_channel);
        overlay.accounts.insert(ALICE, account(250));
        overlay.accounts.insert(BOB, account(350));
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn a_closed_channel_paying_out_more_than_it_escrowed_is_refused() {
        let (db, _dir) = open();
        let mut open_channel = ChannelRecord::new(ALICE, BOB, 600, 10);
        db.put_channel(&[9u8; 32], &open_channel).expect("escrow");

        open_channel.status = ChannelStatus::Closed;
        let mut overlay = Overlay::default();
        overlay.channels.insert([9u8; 32], open_channel);
        overlay.accounts.insert(ALICE, account(600));
        overlay.accounts.insert(BOB, account(600));
        assert!(violation(&db, &overlay).contains("out by 600"));
    }

    #[test]
    fn a_pool_whose_reserves_rise_unpaid_is_refused() {
        // The synthetic form of every AMM accounting bug: a reserve grows and
        // no holder paid for the growth.
        let (db, _dir) = open();
        // Share counts clear `MINIMUM_LIQUIDITY`, because `holdable_shares`
        // saturates below it: a pool seeded with fewer shares than the floor
        // cannot exist, and a fixture that used one would make every share
        // term silently zero and the test pass without checking anything.
        db.raw_put(&pool_key(&PAIR), &pool_record(1_000, 2_000, 2_000).encode())
            .expect("seed pool");

        let mut overlay = Overlay::default();
        StateDB::put_record(
            &mut overlay,
            pool_key(&PAIR),
            pool_record(1_500, 2_000, 2_000).encode(),
        );
        assert!(violation(&db, &overlay).contains("out by 500"));
    }

    #[test]
    fn a_deposit_that_pays_for_the_reserve_it_adds_conserves() {
        let (db, _dir) = open();
        db.raw_put(&pool_key(&PAIR), &pool_record(1_000, 2_000, 2_000).encode())
            .expect("seed pool");
        db.put_account(&ALICE, &account(500)).expect("fund");
        db.raw_put(&balance_key(&QUOTE, &ALICE), &encode_balance(1_000))
            .expect("seed balance");

        // Half the reserves added, so half again the shares: 2,000 -> 3,000
        // issued, of which 1,000 are the locked `MINIMUM_LIQUIDITY` on each
        // side, so the *holdable* supply goes 1,000 -> 2,000 and exactly 1,000
        // new shares have to reach a holder.
        let mut overlay = Overlay::default();
        StateDB::put_record(
            &mut overlay,
            pool_key(&PAIR),
            pool_record(1_500, 3_000, 3_000).encode(),
        );
        overlay.accounts.insert(ALICE, account(0));
        StateDB::put_record(
            &mut overlay,
            balance_key(&QUOTE, &ALICE),
            encode_balance(0).to_vec(),
        );
        // The shares the pool issued have to land somewhere, or the pool has
        // issued value nobody holds.
        StateDB::put_record(
            &mut overlay,
            balance_key(&derive_lp_asset(&PAIR), &ALICE),
            encode_balance(1_000).to_vec(),
        );
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn creating_a_pool_conserves_even_though_minimum_liquidity_reaches_nobody() {
        // The case `holdable_shares` exists for. A pool's first deposit mints
        // `MINIMUM_LIQUIDITY` shares to nobody on purpose, so `total_shares`
        // is permanently that much larger than the sum of every share balance.
        // An equation that did not subtract them would refuse the creation of
        // every pool on this chain — so this test fails the moment somebody
        // "simplifies" that subtraction away.
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");
        db.raw_put(&balance_key(&QUOTE, &ALICE), &encode_balance(2_000))
            .expect("seed balance");

        let mut overlay = Overlay::default();
        StateDB::put_record(
            &mut overlay,
            pool_key(&PAIR),
            pool_record(1_000, 2_000, 2_000).encode(),
        );
        overlay.accounts.insert(ALICE, account(0));
        StateDB::put_record(
            &mut overlay,
            balance_key(&QUOTE, &ALICE),
            encode_balance(0).to_vec(),
        );
        // 2,000 issued, 1,000 locked, so 1,000 reach the depositor.
        StateDB::put_record(
            &mut overlay,
            balance_key(&derive_lp_asset(&PAIR), &ALICE),
            encode_balance(1_000).to_vec(),
        );
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn a_pool_crediting_more_shares_than_it_issued_above_the_floor_is_refused() {
        // The same creation, but the depositor is credited the whole
        // `total_shares` rather than what is left after the floor. If
        // `holdable_shares` ever stopped subtracting, this is the test that
        // notices: it would start passing.
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");
        db.raw_put(&balance_key(&QUOTE, &ALICE), &encode_balance(2_000))
            .expect("seed balance");

        let mut overlay = Overlay::default();
        StateDB::put_record(
            &mut overlay,
            pool_key(&PAIR),
            pool_record(1_000, 2_000, 2_000).encode(),
        );
        overlay.accounts.insert(ALICE, account(0));
        StateDB::put_record(
            &mut overlay,
            balance_key(&QUOTE, &ALICE),
            encode_balance(0).to_vec(),
        );
        StateDB::put_record(
            &mut overlay,
            balance_key(&derive_lp_asset(&PAIR), &ALICE),
            encode_balance(2_000).to_vec(),
        );
        assert!(violation(&db, &overlay).contains("out by 1000"));
    }

    #[test]
    fn shares_credited_without_the_pool_issuing_them_are_refused() {
        let (db, _dir) = open();
        let lp = derive_lp_asset(&PAIR);

        let mut overlay = Overlay::default();
        StateDB::put_record(
            &mut overlay,
            balance_key(&lp, &ALICE),
            encode_balance(50).to_vec(),
        );
        assert!(violation(&db, &overlay).contains(&hex::encode(lp)));
    }

    #[test]
    fn a_registered_assets_whole_supply_must_reach_its_creator() {
        let (db, _dir) = open();
        let asset = [5u8; 32];
        let record = AssetRecord {
            creator: ALICE,
            total_supply: 1_000,
            symbol: *b"USD\0\0\0\0\0",
        };

        let mut overlay = Overlay::default();
        StateDB::put_record(&mut overlay, asset_key(&asset), record.encode().to_vec());
        // Registration alone, with nobody credited, is a thousand units issued
        // and nowhere held.
        assert!(violation(&db, &overlay).contains("out by -1000"));

        StateDB::put_record(
            &mut overlay,
            balance_key(&asset, &ALICE),
            encode_balance(1_000).to_vec(),
        );
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn the_shielded_pool_gaining_without_a_transparent_debit_is_refused() {
        let (db, _dir) = open();
        let mut pool = ShieldedPool::new();
        pool.settle(500, 0, 0).expect("settle");

        let overlay = Overlay {
            shielded: Some(pool),
            ..Default::default()
        };
        assert!(violation(&db, &overlay).contains("out by 500"));
    }

    #[test]
    fn shielding_conserves_when_the_transparent_side_pays() {
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");
        let mut pool = ShieldedPool::new();
        pool.settle(500, 0, 0).expect("settle");

        let mut overlay = Overlay {
            shielded: Some(pool),
            ..Default::default()
        };
        overlay.accounts.insert(ALICE, account(500));
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn a_governance_lock_is_the_coin_it_debited() {
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");

        let mut overlay = Overlay::default();
        overlay.accounts.insert(ALICE, account(700));
        StateDB::put_record(
            &mut overlay,
            crate::governance::lock_key(&ALICE),
            LockRecord {
                amount: 300,
                unlock_height: 100,
            }
            .encode()
            .to_vec(),
        );
        db.check_value_conservation(&overlay).expect("conserved");
    }

    #[test]
    fn a_lock_that_credits_weight_it_never_paid_for_is_refused() {
        let (db, _dir) = open();
        db.put_account(&ALICE, &account(1_000)).expect("fund");

        let mut overlay = Overlay::default();
        StateDB::put_record(
            &mut overlay,
            crate::governance::lock_key(&ALICE),
            LockRecord {
                amount: 300,
                unlock_height: 100,
            }
            .encode()
            .to_vec(),
        );
        assert!(violation(&db, &overlay).contains("out by 300"));
    }
}
