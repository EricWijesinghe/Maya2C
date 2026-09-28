//! **SIM.** Absorbing a renewable surge by scaling useful-work compute up,
//! as Master Prompt 6 §8 describes: when generation exceeds demand, miners
//! running proof-of-useful-work take the surplus as load.
//!
//! This models only the decision — which miners take how much — and how
//! long it takes to make. Whether real miners can ramp that fast, and what
//! the grid does meanwhile, is not modelled.

/// A miner's spare capacity, in kW.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Miner {
    /// Its id.
    pub id: u32,
    /// How much more load it can take, kW.
    pub headroom_kw: u64,
}

/// The allocation: each miner's added load, and any surplus left over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    /// `(miner, kW)`, largest headroom first.
    pub loads: Vec<(u32, u64)>,
    /// kW nobody could take.
    pub unabsorbed_kw: u64,
}

/// Allocates `surplus_kw` across `miners`, largest headroom first, so the
/// fewest miners change state. Deterministic: ties go to the lower id.
#[must_use]
pub fn absorb(surplus_kw: u64, miners: &[Miner]) -> Allocation {
    let mut order: Vec<&Miner> = miners.iter().collect();
    order.sort_by(|a, b| b.headroom_kw.cmp(&a.headroom_kw).then(a.id.cmp(&b.id)));
    let mut left = surplus_kw;
    let mut loads = Vec::new();
    for m in order {
        if left == 0 {
            break;
        }
        let take = m.headroom_kw.min(left);
        if take > 0 {
            loads.push((m.id, take));
            left -= take;
        }
    }
    Allocation {
        loads,
        unabsorbed_kw: left,
    }
}
