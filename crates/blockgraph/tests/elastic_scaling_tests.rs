//! Elastic shards under simulated load.
//!
//! A network of lanes starts on four shards, runs at peak until the policy
//! has bisected every range down to 64, then goes idle until buddy merges
//! bring it back to four. Around that: the hysteresis that keeps a split from
//! thrashing, the vetoes, a ground address prefix, and — the claim everything
//! else rests on — that the waves reach the serial state under every map the
//! network passes through.
//!
//! Load is expressed against a lane capacity of [`LANE`] transactions per tick,
//! so nothing here depends on the speed of the machine running it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use maya_blockgraph::shard_manager::{
    Decision, Prefix, ScalingConfig, ShardManager, ShardMap, ShardStores, TickSample,
};
use maya_blockgraph::{ShardId, schedule};

/// Transactions one lane executes per tick.
const LANE: u32 = 20;
/// Enough to hold every leaf of a 64-leaf map above 80% of a lane.
const PEAK: usize = 1_600;
/// Few enough that every buddy pair is under 40% of a lane (8 transactions)
/// even if every idle transaction lands in it. At 16, two quarters' halves
/// carry 8 between them — exactly the line, never under it — and the collapse
/// stalls at the last merge.
const IDLE: usize = 4;
/// Records cached across the lanes, moved at every rebalance.
const RECORDS: usize = 2_000;
/// A quiet machine.
const LOW_PRESSURE: u16 = 100;

fn config() -> ScalingConfig {
    ScalingConfig {
        lane_capacity: LANE,
        ..ScalingConfig::DEFAULT
    }
}

/// xorshift64: reproducible across machines, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn address(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next().to_le_bytes());
        }
        out
    }

    /// An address sharing the first 16 key bits of `near`: the same leaf at
    /// any depth up to 16, so never a straddle.
    fn neighbour(&mut self, near: &[u8; 32]) -> [u8; 32] {
        let mut out = self.address();
        out[..2].copy_from_slice(&near[..2]);
        out
    }

    /// An address whose first `depth` key bits are those of `bits`.
    fn under(&mut self, bits: u32, depth: u32) -> [u8; 32] {
        let mut out = self.address();
        let free = u32::from_be_bytes([out[0], out[1], out[2], out[3]]);
        let fixed = if depth == 0 {
            0
        } else {
            u32::MAX << (32 - depth)
        };
        out[..4].copy_from_slice(&((bits & fixed) | (free & !fixed)).to_be_bytes());
        out
    }
}

/// `count` transactions, alternately one address and two neighbours.
fn local_load(rng: &mut Rng, count: usize) -> Vec<Vec<[u8; 32]>> {
    (0..count)
        .map(|n| {
            let first = rng.address();
            if n % 2 == 0 {
                vec![first]
            } else {
                vec![first, rng.neighbour(&first)]
            }
        })
        .collect()
}

/// One rebalance: the tick, the shard count after it, what changed.
type Event = (u64, usize, Vec<Decision>);

struct Network {
    manager: ShardManager,
    stores: ShardStores<usize>,
    keys: Vec<[u8; 32]>,
    history: Vec<Event>,
}

impl Network {
    fn new(config: ScalingConfig, map: ShardMap, rng: &mut Rng) -> Self {
        let mut stores = ShardStores::new(map.clone());
        let keys: Vec<[u8; 32]> = (0..RECORDS).map(|_| rng.address()).collect();
        for (value, key) in keys.iter().enumerate() {
            stores.insert(*key, value);
        }
        Self {
            manager: ShardManager::new(config, map).expect("manager"),
            stores,
            keys,
            history: Vec::new(),
        }
    }

    fn shards(&self) -> usize {
        self.manager.map().shard_count()
    }

    /// Runs one tick; returns the new shard count if the map changed.
    fn tick(&mut self, transactions: &[Vec<[u8; 32]>], pressure: u16) -> Option<usize> {
        let views: Vec<&[[u8; 32]]> = transactions.iter().map(Vec::as_slice).collect();
        let sample = TickSample::measure(self.manager.map(), &views, pressure).expect("sample");
        let plan = self.manager.observe(&sample).expect("observe")?;

        let relocations = self.stores.apply(&plan).expect("handoff");
        let moved: usize = relocations.iter().map(|run| run.records).sum();
        assert_eq!(
            moved,
            RECORDS,
            "tick {}: a record was lost or doubled",
            plan.tick()
        );
        assert_eq!(self.stores.record_count(), RECORDS);

        self.history.push((
            plan.tick(),
            plan.to().shard_count(),
            plan.decisions().to_vec(),
        ));
        self.manager.commit(plan).expect("commit");
        assert_eq!(self.stores.map(), self.manager.map());
        Some(self.shards())
    }

    fn assert_every_record_is_where_its_key_says(&self) {
        for (value, key) in self.keys.iter().enumerate() {
            assert_eq!(self.stores.get(key), Some(&value));
        }
    }
}

fn counts(events: &[Event]) -> Vec<usize> {
    events.iter().map(|event| event.1).collect()
}

#[test]
fn a_network_scales_from_four_shards_to_sixty_four_at_peak_and_back_to_four_when_idle() {
    let mut rng = Rng(0x5EED_CAFE);
    let mut network = Network::new(config(), ShardMap::uniform(2).expect("four"), &mut rng);

    let mut ticks = 0;
    while network.shards() < 64 {
        assert!(
            ticks < 1_000,
            "stuck at {} shards after {:?}",
            network.shards(),
            network.history
        );
        network.tick(&local_load(&mut rng, PEAK), LOW_PRESSURE);
        ticks += 1;
    }
    assert_eq!(counts(&network.history), [8, 16, 32, 64]);
    assert_eq!(
        network.history[0].0, 100,
        "the 100th consecutive hot tick splits"
    );
    let splits_only = network.history.iter().flat_map(|event| &event.2);
    assert!(splits_only.clone().all(|d| matches!(d, Decision::Split(_))));
    assert!(
        network
            .manager
            .map()
            .leaves()
            .iter()
            .all(|leaf| leaf.depth() == 6)
    );

    for _ in 0..300 {
        assert_eq!(
            network.tick(&local_load(&mut rng, PEAK), LOW_PRESSURE),
            None
        );
    }
    network.assert_every_record_is_where_its_key_says();

    let grown = network.history.len();
    let mut idle_ticks = 0;
    while network.shards() > 4 {
        assert!(
            idle_ticks < 3_000,
            "stuck at {} shards after {:?}",
            network.shards(),
            counts(&network.history)
        );
        network.tick(&local_load(&mut rng, IDLE), LOW_PRESSURE);
        idle_ticks += 1;
    }
    let shrink = &network.history[grown..];
    assert_eq!(counts(shrink), [32, 16, 8, 4]);
    assert!(
        shrink
            .iter()
            .flat_map(|event| &event.2)
            .all(|d| matches!(d, Decision::Merge(_)))
    );

    for _ in 0..1_000 {
        assert_eq!(
            network.tick(&local_load(&mut rng, IDLE), LOW_PRESSURE),
            None
        );
    }
    assert_eq!(network.manager.map(), &ShardMap::uniform(2).expect("four"));
    network.assert_every_record_is_where_its_key_says();
}

/// `per_leaf[i]` single-address transactions under the `i`-th quarter of the
/// space, alternating between its halves so none straddles.
fn quarter_load(rng: &mut Rng, per_leaf: [usize; 4]) -> Vec<Vec<[u8; 32]>> {
    let mut out = Vec::new();
    for (quarter, count) in per_leaf.into_iter().enumerate() {
        for n in 0..count {
            let bits = ((quarter as u32) << 30) | (((n % 2) as u32) << 29);
            out.push(vec![rng.under(bits, 3)]);
        }
    }
    out
}

#[test]
fn load_hovering_at_the_split_line_never_thrashes() {
    let mut rng = Rng(0x0000_7E57);
    let mut network = Network::new(config(), ShardMap::uniform(2).expect("four"), &mut rng);
    // 17 is 85% of a lane; every hundredth tick drops to 16, exactly 80%,
    // which is not *above* the line and resets every streak.
    for tick in 0..2_000 {
        let per_leaf = if tick % 100 == 99 { 16 } else { 17 };
        network.tick(&quarter_load(&mut rng, [per_leaf; 4]), LOW_PRESSURE);
    }
    assert!(network.history.is_empty(), "{:?}", network.history);
}

#[test]
fn a_split_sticks_because_its_children_start_above_the_merge_line() {
    let mut rng = Rng(0x0000_5711);
    let mut network = Network::new(config(), ShardMap::uniform(2).expect("four"), &mut rng);
    // Quarter 0 at 120% of a lane, the rest at 50%. The split halves quarter 0
    // into two leaves at 60%, which together are far above the 40% merge line.
    for _ in 0..3_000 {
        network.tick(&quarter_load(&mut rng, [24, 10, 10, 10]), LOW_PRESSURE);
    }
    let quarter = Prefix::new(0, 2).expect("canonical");
    assert_eq!(network.history, [(100, 5, vec![Decision::Split(quarter)])]);
}

#[test]
fn a_ground_address_prefix_deepens_only_its_own_range_and_stops_at_sixty_four() {
    let config = ScalingConfig {
        split_streak: 10,
        cooldown_ticks: 10,
        ..config()
    };
    let mut rng = Rng(0x0BAD_5EED);
    let mut network = Network::new(config, ShardMap::uniform(2).expect("four"), &mut rng);
    // Every address shares its first 12 key bits: 0xABC, under quarter 0b10.
    let hot_bits = 0xABC0_0000;
    for _ in 0..1_500 {
        let load: Vec<Vec<[u8; 32]>> = (0..800).map(|_| vec![rng.under(hot_bits, 12)]).collect();
        network.tick(&load, LOW_PRESSURE);
        assert!(network.shards() <= 64);
    }

    let hot_quarter = Prefix::new(0x8000_0000, 2).expect("canonical");
    let leaves = network.manager.map().leaves();
    for leaf in leaves {
        let inside = leaf.depth() >= 2 && leaf.bits() & 0xC000_0000 == hot_quarter.bits();
        assert!(
            inside || leaf.depth() <= 2,
            "{leaf:?} split outside the hot range"
        );
    }
    assert!(leaves.iter().any(|leaf| leaf.depth() > 12));
}

#[test]
fn memory_pressure_holds_splits_until_it_eases() {
    let mut rng = Rng(0x0000_3E3E_3E3E_3E3E);
    let mut network = Network::new(config(), ShardMap::uniform(2).expect("four"), &mut rng);
    for _ in 0..500 {
        assert_eq!(network.tick(&local_load(&mut rng, 400), 950), None);
    }
    assert_eq!(
        network.tick(&local_load(&mut rng, 400), LOW_PRESSURE),
        Some(8)
    );
    assert_eq!(network.history[0].0, 501);
}

#[test]
fn load_straddling_both_halves_of_its_leaf_is_not_split() {
    let mut rng = Rng(0x57AD_D1E5);
    let mut network = Network::new(config(), ShardMap::uniform(2).expect("four"), &mut rng);
    for _ in 0..500 {
        let load: Vec<Vec<[u8; 32]>> = (0..200u32)
            .map(|n| {
                let quarter = (n % 4) << 30;
                vec![rng.under(quarter, 3), rng.under(quarter | 1 << 29, 3)]
            })
            .collect();
        network.tick(&load, LOW_PRESSURE);
    }
    assert!(network.history.is_empty(), "{:?}", network.history);
}

type Model = BTreeMap<[u8; 32], u64>;

/// Folds transaction `index` into every address it names. Order-sensitive,
/// as in `serial_equivalence_tests.rs`, so a reordering is visible.
fn apply(state: &mut Model, index: usize, addresses: &[[u8; 32]]) {
    for address in addresses {
        let slot = state.entry(*address).or_insert(0);
        *slot = slot
            .rotate_left(7)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(index as u64 + 1);
    }
}

fn serial(transactions: &[Vec<[u8; 32]>]) -> Model {
    let mut state = Model::new();
    for (index, addresses) in transactions.iter().enumerate() {
        apply(&mut state, index, addresses);
    }
    state
}

/// Waves under `map`, each applied in reverse.
fn in_waves(map: &ShardMap, transactions: &[Vec<[u8; 32]>]) -> Model {
    let accesses: Vec<_> = transactions
        .iter()
        .map(|addresses| map.access_for(addresses).expect("access"))
        .collect();
    let mut state = Model::new();
    for wave in schedule(&accesses).expect("schedule") {
        for &index in wave.indices().iter().rev() {
            apply(&mut state, index, &transactions[index]);
        }
    }
    state
}

#[test]
fn waves_reach_the_serial_state_under_every_map_the_network_passes_through() {
    let mut rng = Rng(0x00E9_0A11);
    let pool: Vec<[u8; 32]> = (0..256).map(|_| rng.address()).collect();
    let mut network = Network::new(config(), ShardMap::uniform(2).expect("four"), &mut rng);

    let mut maps_checked = 0;
    for tick in 0..1_000u64 {
        let load: Vec<Vec<[u8; 32]>> = (0..900)
            .map(|n| {
                let width = if n % 5 == 0 { 3 } else { 1 };
                (0..width)
                    .map(|_| pool[(rng.next() % 256) as usize])
                    .collect()
            })
            .collect();
        if tick % 25 == 0 || network.history.last().is_some_and(|event| event.0 == tick) {
            assert_eq!(
                in_waves(network.manager.map(), &load),
                serial(&load),
                "tick {tick}, {} shards",
                network.shards()
            );
            maps_checked += 1;
        }
        network.tick(&load, LOW_PRESSURE);
    }
    assert_eq!(network.shards(), 64, "the run should reach every depth");
    assert!(maps_checked >= 40);
}

#[test]
fn transactions_sharing_an_address_conflict_under_every_map() {
    let mut rng = Rng(0x00C0_FF1C);
    let mut map = ShardMap::uniform(0).expect("one leaf");
    for _ in 0..63 {
        let target = ShardId::new((rng.next() % map.shard_count() as u64) as usize).expect("id");
        map = map.split(target).unwrap_or(map);
        for _ in 0..200 {
            let shared = rng.address();
            let left = map.access_for(&[shared, rng.address()]).expect("access");
            let right = map.access_for(&[rng.address(), shared]).expect("access");
            assert!(left.conflicts_with(right));
        }
    }
}
