//! An in-process fake of the public RPC, for the scenario tests. Its values are
//! written out by hand, so a test that breaks one figure shows which check
//! notices it.

use std::collections::BTreeMap;

use maya_amount::Amount;

use crate::genesis::{MiningParams, Plan};
use crate::numbers::{FEE_COLLECTOR, FEE_SINK, MINING_RESERVE};
use crate::rpc::{ChainFacts, Change, Output, Rpc, RpcError, SupplyFacts};

#[derive(Clone, Default)]
pub struct Fake {
    pub tip: u64,
    pub supply_now: SupplyFacts,
    pub reserve_now: Amount,
    pub changes: BTreeMap<u64, Vec<Change>>,
    pub outputs: BTreeMap<u64, Vec<Output>>,
    pub reserve_at: BTreeMap<u64, Amount>,
    /// Heights whose balance changes the node no longer keeps.
    pub refuse_changes: Vec<u64>,
    /// Every call fails at the transport level.
    pub broken: bool,
}

impl Rpc for Fake {
    fn chain_info(&self) -> Result<ChainFacts, RpcError> {
        self.transport()?;
        Ok(ChainFacts {
            genesis: "00".repeat(32),
            height: self.tip,
        })
    }

    fn supply(&self) -> Result<SupplyFacts, RpcError> {
        self.transport()?;
        Ok(self.supply_now)
    }

    fn balance(&self, address: &[u8; 32]) -> Result<Amount, RpcError> {
        self.transport()?;
        if *address == MINING_RESERVE {
            Ok(self.reserve_now)
        } else {
            Err(RpcError::Rejected("fake knows only the reserve".into()))
        }
    }

    fn balance_at(&self, address: &[u8; 32], height: u64) -> Result<Amount, RpcError> {
        self.transport()?;
        match (address == &MINING_RESERVE, self.reserve_at.get(&height)) {
            (true, Some(value)) => Ok(*value),
            _ => Err(RpcError::Rejected(format!("no balance at {height}"))),
        }
    }

    fn balance_changes(&self, height: u64) -> Result<Vec<Change>, RpcError> {
        self.transport()?;
        if self.refuse_changes.contains(&height) {
            return Err(RpcError::Rejected(format!(
                "no balance changes kept for height {height}"
            )));
        }
        Ok(self.changes.get(&height).cloned().unwrap_or_default())
    }

    fn block_outputs(&self, height: u64) -> Result<Vec<Output>, RpcError> {
        self.transport()?;
        Ok(self.outputs.get(&height).cloned().unwrap_or_default())
    }
}

impl Fake {
    fn transport(&self) -> Result<(), RpcError> {
        if self.broken {
            Err(RpcError::Transport("connection refused".into()))
        } else {
            Ok(())
        }
    }

    /// Records one reserve payout (`before -> after`) at `height`.
    pub fn reserve_move(&mut self, height: u64, before: u64, after: u64) {
        self.push(height, MINING_RESERVE, before, after);
    }

    /// Records a balance change for any account.
    pub fn push(&mut self, height: u64, address: [u8; 32], before: u64, after: u64) {
        self.changes.entry(height).or_default().push(Change {
            address,
            before: Amount::from_u64(before),
            after: Amount::from_u64(after),
        });
    }

    /// Records a transaction output in `height`'s block.
    pub fn output(&mut self, height: u64, recipient: [u8; 32], amount: u64) {
        self.outputs.entry(height).or_default().push(Output {
            recipient,
            amount: Amount::from_u64(amount),
        });
    }
}

/// The chain the scenario tests share: genesis reserve 1,000,000 and 5,000 to
/// a user, so the genesis total is 1,005,000. Epochs of 4 blocks, reward 1000
/// halving every 2 epochs with a tail of 100, miners' share 70%.
pub fn plan() -> Plan {
    Plan {
        genesis_total: Amount::from_u64(1_005_000),
        reserve_genesis: Amount::from_u64(1_000_000),
        mining: Some(MiningParams {
            initial_epoch_reward: Amount::from_u64(1000),
            tail_epoch_reward: Amount::from_u64(100),
            halving_epochs: 2,
            miner_share_bps: 7000,
        }),
        epoch_blocks: Some(4),
    }
}

/// Twelve blocks, three epochs, every figure derived by hand:
///
/// - epoch 0 (heights 1-4): reward 1000 = committee 300 + miners 700.
///   Shares 200 and 250, committee 300 at height 4: 750 out, 450 of budget used.
/// - epoch 1 (heights 5-8): reward 1000. Share 100, committee 300: 400 out.
/// - epoch 2 (heights 9-12): reward 500 = committee 150 + miners 350.
///   Share 350, committee 150 at height 12: 500 out, budget used in full.
///
/// Totals: paid 1650, unspent miner budget 850, scheduled 2500 (= 1650 + 850).
/// Reserve 1,000,000 - 1,650 = 998,350. Supply stays at the genesis total.
/// Fees: height 2 collects 10 and burns 4 (collector +6); height 6 collects 7
/// and burns 3 (collector +4).
pub fn chain() -> Fake {
    let mut f = Fake {
        tip: 12,
        ..Fake::default()
    };
    f.reserve_move(1, 1_000_000, 999_800);
    f.reserve_move(2, 999_800, 999_550);
    f.push(2, FEE_COLLECTOR, 100, 106);
    f.push(2, FEE_SINK, 0, 4);
    f.output(2, FEE_COLLECTOR, 10);
    f.reserve_move(4, 999_550, 999_250);
    f.reserve_move(5, 999_250, 999_150);
    f.push(6, FEE_COLLECTOR, 50, 54);
    f.push(6, FEE_SINK, 4, 7);
    f.output(6, FEE_COLLECTOR, 7);
    f.reserve_move(8, 999_150, 998_850);
    f.reserve_move(9, 998_850, 998_500);
    f.reserve_move(12, 998_500, 998_350);
    f.reserve_at.insert(4, Amount::from_u64(999_250));
    f.reserve_at.insert(8, Amount::from_u64(998_850));
    f.reserve_at.insert(12, Amount::from_u64(998_350));
    f.reserve_now = Amount::from_u64(998_350);
    f.supply_now = SupplyFacts {
        total: Amount::from_u64(1_005_000),
        shielded: Amount::ZERO,
        burned: Amount::from_u64(7),
    };
    f
}
