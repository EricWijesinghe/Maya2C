//! `cargo xtask genesis-check <genesis.json>` — mainnet genesis policy, from the file.
//!
//! Decided 2026-10-25: no operator holds more than 20% of the bonded stake,
//! at least 7 validators, stake-weighted voting, and a real proof-of-work
//! difficulty. Concentration is counted per **operator**, not per bond: one
//! operator running four validators is one share (the hybrid testnet is
//! exactly that case, and fails here by design). Hosts cannot be read from a
//! genesis file, so "3 or more hosts" stays a human check in `go-no-go`.
//!
//! Proof of work: on a DAG-BFT chain blocks carry none (the ceremony writes
//! `difficulty_bits = 0`); the work is in the mining shares (`bft.mining`,
//! ADR-044), so a chain with mining passes at difficulty 0 and one without
//! mining needs a block difficulty. The fee floor must be at least
//! [`MIN_BASE_FEE`] (pre-audit S-007).

use maya_amount::Amount;
use serde::Deserialize;
use std::collections::BTreeMap;

const MAX_SHARE_BPS: u128 = 2_000;
const MIN_VALIDATORS: usize = 7;
/// Lowest mainnet base fee per byte: 1 unit at 18 decimals, 10^6 at 24.
const MIN_BASE_FEE: u128 = 1_000_000;
/// Mainnet staking floors (24 decimals): 1,000 coins to register a validator
/// and 1 coin per delegation, decided 2026-10-30 (DECISIONS.md) under Eric's
/// delegation; changing either is one line here. The devnet defaults (1,000 and
/// 10 base units) are dust: a free validator seat, a free delegation.
pub(crate) const MIN_SELF_BOND: u128 = 1_000 * COIN;
const MIN_DELEGATION: u128 = COIN;
const COIN: u128 = maya_amount::ONE_COIN;

/// The keyless mining reserve's address: the ASCII tag below, zero-padded to 32
/// bytes. `crates/node/src/state/mining.rs` holds the same bytes, and a test
/// here pins them, because xtask does not link the node.
const MINING_RESERVE_TAG: &[u8] = b"maya2c/mining/reserve/v1";
/// A genesis older than this was written for a different launch.
const MAX_GENESIS_AGE_SECS: u64 = 365 * 86_400;
/// Signing happens weeks before launch, so the timestamp may lead the check.
const MAX_GENESIS_LEAD_SECS: u64 = 90 * 86_400;

#[derive(Deserialize)]
struct Genesis {
    difficulty_bits: u32,
    /// Absent means the pool runs from block zero (ADR-037).
    #[serde(default)]
    shielded_activation_height: Option<u64>,
    /// Unix seconds, checked against the clock the caller passes in.
    timestamp: u64,
    /// Premined balances. Mainnet allows exactly one: the keyless reserve.
    #[serde(default)]
    allocations: Vec<Allocation>,
    /// Scheduled protocol upgrades. None may be pending at launch.
    #[serde(default)]
    protocol_upgrades: Vec<serde_json::Value>,
    bft: Bft,
}

#[derive(Deserialize)]
struct Allocation {
    address: String,
    /// Kept as written, not parsed: the reserve (about 1.2e41) is past `u128`.
    balance: serde_json::Value,
}

/// Least anchor timeout: the node's `MIN_ANCHOR_TIMEOUT_MS`
/// (crates/node/src/genesis.rs, which explains the number). Repeated here
/// because xtask does not link the node.
const MIN_ANCHOR_TIMEOUT_MS: u64 = 500;

/// The node's default when a genesis leaves the field out.
const fn default_anchor_timeout_ms() -> u64 {
    1_000
}

/// The node's `CANONICAL_ROUND_INTERVAL_MS` (crates/node/src/genesis.rs): one
/// block per second, the rate every block-counted governance window assumes.
/// Also the node's default, so an absent field passes.
const CANONICAL_ROUND_INTERVAL_MS: u64 = 500;

const fn default_round_interval_ms() -> u64 {
    CANONICAL_ROUND_INTERVAL_MS
}

#[derive(Deserialize)]
struct Bft {
    validators: Vec<String>,
    #[serde(default = "default_anchor_timeout_ms")]
    anchor_timeout_ms: u64,
    #[serde(default = "default_round_interval_ms")]
    round_interval_ms: u64,
    staking: Staking,
    #[serde(default)]
    fees: Option<Fees>,
    #[serde(default)]
    mining: Option<Mining>,
}

#[derive(Deserialize)]
struct Mining {
    /// Zero or absent means the reward stops at the halving floor (ADR-068).
    #[serde(default, deserialize_with = "units_opt")]
    tail_epoch_reward: Option<u128>,
    /// Kept as written, like an allocation balance; read by [`amount_of`].
    #[serde(default)]
    initial_epoch_reward: serde_json::Value,
    #[serde(default)]
    halving_epochs: Option<u64>,
}

#[derive(Deserialize)]
struct Fees {
    #[serde(deserialize_with = "units")]
    min_base_fee: u128,
}

/// A 256-bit amount as genesis writes it, a JSON number or a decimal string,
/// read as `u128`: every amount this policy compares fits, and one that does
/// not is refused rather than wrapped.
fn units<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_u64()
            .map(u128::from)
            .ok_or_else(|| serde::de::Error::custom(format!("{n} is not a whole amount"))),
        serde_json::Value::String(text) => text.parse().map_err(serde::de::Error::custom),
        other => Err(serde::de::Error::custom(format!(
            "{other} is not an amount"
        ))),
    }
}

#[derive(Deserialize)]
struct Staking {
    #[serde(default)]
    stake_weighted: bool,
    bonds: Vec<Bond>,
    /// Absent means the node's devnet default, which fails the floor.
    #[serde(default, deserialize_with = "units_opt")]
    min_self_bond: Option<u128>,
    #[serde(default, deserialize_with = "units_opt")]
    min_delegation: Option<u128>,
    /// Prices the mining reserve: the schedule counts epochs to the last height.
    #[serde(default)]
    epoch_blocks: Option<u64>,
}

fn units_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u128>, D::Error> {
    units(d).map(Some)
}

#[derive(Deserialize)]
struct Bond {
    operator: String,
    #[serde(deserialize_with = "units")]
    bond: u128,
}

/// The reserve address as genesis writes it: lowercase hex, no `0x`.
fn mining_reserve_address() -> String {
    let mut padded = [0u8; 32];
    padded[..MINING_RESERVE_TAG.len()].copy_from_slice(MINING_RESERVE_TAG);
    hex::encode(padded)
}

/// A genesis amount written as a decimal string or a JSON integer, at full
/// 256 bits; `None` for anything else or an overflow.
pub(crate) fn amount_of(value: &serde_json::Value) -> Option<Amount> {
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.bytes().try_fold(Amount::ZERO, |acc, digit| {
        acc.checked_mul(Amount::from_u64(10))?
            .checked_add(Amount::from_u64(u64::from(digit - b'0')))
    })
}

/// `MiningRecord::lifetime_emission` (`crates/node/src/state/mining.rs`),
/// re-derived so the xtask stays off the node crate; a test pins it to the
/// mainnet figure the node computes (`deploy/genesis/mainnet/README.md`).
/// Everything the schedule pays, epoch 0 to the last epoch a `u64` height
/// reaches; `None` at or past the node's 2^192 ceiling.
fn lifetime_emission(
    initial: Amount,
    tail: Amount,
    halving_epochs: u64,
    epoch_blocks: u64,
) -> Option<Amount> {
    if halving_epochs == 0 || epoch_blocks == 0 {
        return None;
    }
    let epochs_total = u128::from(u64::MAX / epoch_blocks) + 1;
    let era = u128::from(halving_epochs);
    let mut total = Amount::ZERO;
    let mut start: u128 = 0;
    for halvings in 0u32..=256 {
        if start >= epochs_total {
            break;
        }
        let halved = initial.shr(halvings);
        let left = epochs_total - start;
        if halved <= tail {
            total = total.checked_add(tail.checked_mul(Amount::from_u128(left))?)?;
            break;
        }
        total = total.checked_add(halved.checked_mul(Amount::from_u128(era.min(left)))?)?;
        start = start.saturating_add(era);
    }
    (total < LIFETIME_EMISSION_CEILING).then_some(total)
}

/// The `--mining-reserve` a `mining.json` needs at `epoch_blocks`: its
/// lifetime emission, by the rule [`reserve_rule`] checks, so a rehearsal
/// never copies the figure by hand.
pub(crate) fn mining_reserve(mining_json: &str, epoch_blocks: u64) -> Result<String, String> {
    let m: Mining = serde_json::from_str(mining_json).map_err(|e| format!("mining.json: {e}"))?;
    let initial = amount_of(&m.initial_epoch_reward).ok_or("mining.json: initial_epoch_reward")?;
    let tail = Amount::from_u128(m.tail_epoch_reward.unwrap_or(0));
    let halving = m.halving_epochs.ok_or("mining.json: halving_epochs")?;
    lifetime_emission(initial, tail, halving, epoch_blocks)
        .map(|a| a.to_string())
        .ok_or_else(|| "the schedule's lifetime emission reaches 2^192".to_string())
}

/// The node's `LIFETIME_EMISSION_CEILING`: 2^192.
const LIFETIME_EMISSION_CEILING: Amount = Amount([0, 0, 0, 1]);

/// The reserve must hold exactly the schedule's lifetime emission. The node
/// refuses less; more would be value no rule ever pays out, a premine in
/// all but name.
fn reserve_rule(g: &Genesis) -> Option<String> {
    let mining = g.bft.mining.as_ref()?;
    let reserve = mining_reserve_address();
    let mut funded = Some(Amount::ZERO);
    for a in &g.allocations {
        if normalized_address(&a.address) == reserve {
            funded = funded.and_then(|f| f.checked_add(amount_of(&a.balance)?));
        }
    }
    let Some(funded) = funded else {
        return Some("the mining reserve's balance is not a readable amount".to_string());
    };
    let tail = Amount::from_u128(mining.tail_epoch_reward.unwrap_or(0));
    let lifetime = match (
        amount_of(&mining.initial_epoch_reward),
        mining.halving_epochs,
        g.bft.staking.epoch_blocks,
    ) {
        (Some(initial), Some(halving), Some(epoch)) => {
            lifetime_emission(initial, tail, halving, epoch)
        }
        _ => {
            return Some(
                "bft.mining.initial_epoch_reward, bft.mining.halving_epochs and bft.staking.epoch_blocks are needed to price the mining reserve"
                    .to_string(),
            );
        }
    };
    match lifetime {
        Some(lifetime) if lifetime == funded => None,
        Some(lifetime) => Some(format!(
            "the mining reserve holds {funded} but the schedule's lifetime emission is {lifetime}: it must be exactly that (ADR-044)"
        )),
        None => Some("the mining schedule's lifetime emission reaches 2^192".to_string()),
    }
}

/// `address` as the policy compares it: lowercase hex, optional `0x` dropped.
fn normalized_address(address: &str) -> String {
    address
        .strip_prefix("0x")
        .unwrap_or(address)
        .to_ascii_lowercase()
}

/// Every policy breach in `text`, judged at the clock `now` (Unix seconds);
/// empty means the genesis passes. The clock is a parameter so the window is
/// testable and the policy never reads the wall clock itself.
/// The launch rules that concern money, time and upgrades: no premine, no
/// pending upgrade, a sane timestamp, and a tail. Split out of [`problems`] so
/// each function stays readable.
fn launch_rules(g: &Genesis, now: u64) -> Vec<String> {
    let mut out = Vec::new();
    let reserve = mining_reserve_address();
    for a in &g.allocations {
        if normalized_address(&a.address) != reserve {
            out.push(format!(
                "allocation to {} of {} base units: only the keyless mining reserve may be funded at genesis (no premine, ADR-044)",
                a.address, a.balance
            ));
        }
    }
    if !g.protocol_upgrades.is_empty() {
        out.push(format!(
            "{} protocol upgrade(s) scheduled at genesis: none may be pending at launch",
            g.protocol_upgrades.len()
        ));
    }
    if g.timestamp.saturating_add(MAX_GENESIS_AGE_SECS) < now {
        out.push(format!(
            "timestamp {} is over a year before the check ({now}): a stale genesis",
            g.timestamp
        ));
    } else if g.timestamp > now.saturating_add(MAX_GENESIS_LEAD_SECS) {
        out.push(format!(
            "timestamp {} is over 90 days after the check ({now})",
            g.timestamp
        ));
    }
    match g.bft.mining.as_ref().and_then(|m| m.tail_epoch_reward) {
        Some(tail) if tail > 0 => {}
        _ => out.push(
            "no bft.mining.tail_epoch_reward above zero: rewards would stop at the halving floor (ADR-068)"
                .to_string(),
        ),
    }
    out.extend(reserve_rule(g));
    out
}

fn problems(text: &str, now: u64) -> Result<Vec<String>, String> {
    let g: Genesis = serde_json::from_str(text).map_err(|e| format!("not a genesis file: {e}"))?;
    let mut out = launch_rules(&g, now);
    if g.bft.validators.len() < MIN_VALIDATORS {
        out.push(format!(
            "{} validators, need at least {MIN_VALIDATORS}",
            g.bft.validators.len()
        ));
    }
    if g.bft.anchor_timeout_ms < MIN_ANCHOR_TIMEOUT_MS {
        out.push(format!(
            "bft.anchor_timeout_ms {} is below {MIN_ANCHOR_TIMEOUT_MS}: the resend, re-fetch and fetch-reply limits run at half of it",
            g.bft.anchor_timeout_ms
        ));
    }
    if g.bft.round_interval_ms != CANONICAL_ROUND_INTERVAL_MS {
        out.push(format!(
            "bft.round_interval_ms {} is not {CANONICAL_ROUND_INTERVAL_MS}: the governance and migration windows are counted at one block per second",
            g.bft.round_interval_ms
        ));
    }
    if g.bft.validators.len() != g.bft.staking.bonds.len() {
        out.push("validators and bonds differ in number".to_string());
    }
    let floors = [
        ("min_self_bond", g.bft.staking.min_self_bond, MIN_SELF_BOND),
        (
            "min_delegation",
            g.bft.staking.min_delegation,
            MIN_DELEGATION,
        ),
    ];
    for (name, value, floor) in floors {
        match value {
            Some(v) if v >= floor => {}
            Some(v) => out.push(format!("staking.{name} {v} is below {floor}")),
            None => out.push(format!(
                "staking.{name} is unset: the devnet default is dust"
            )),
        }
    }
    // The tiny bond (Eric 2026-11-07): every seat bonds exactly the minimum,
    // so the ceremony's disclosed bonds are the whole of the stake.
    if let Some(floor) = g.bft.staking.min_self_bond {
        for b in &g.bft.staking.bonds {
            if b.bond != floor {
                out.push(format!(
                    "operator {}… bonds {} but min_self_bond is {floor}: mainnet bonds are exactly the minimum",
                    &b.operator[..b.operator.len().min(12)],
                    b.bond
                ));
            }
        }
    }
    if g.shielded_activation_height != Some(u64::MAX) {
        out.push("shielded pool is not off: its circuit is unaudited (ADR-037)".to_string());
    }
    if !g.bft.staking.stake_weighted {
        out.push("voting is not stake-weighted".to_string());
    }
    if g.difficulty_bits == 0 && g.bft.mining.is_none() {
        out.push("difficulty_bits is 0 and no bft.mining: proof of work is free".to_string());
    }
    match &g.bft.fees {
        None => out.push("no bft.fees: transactions would be free".to_string()),
        Some(f) if f.min_base_fee < MIN_BASE_FEE => out.push(format!(
            "min_base_fee {} is below {MIN_BASE_FEE} (S-007)",
            f.min_base_fee
        )),
        Some(_) => {}
    }
    let mut per_operator: BTreeMap<&str, u128> = BTreeMap::new();
    let mut total: u128 = 0;
    for b in &g.bft.staking.bonds {
        let entry = per_operator.entry(b.operator.as_str()).or_default();
        *entry = entry.saturating_add(b.bond);
        total = total.saturating_add(b.bond);
    }
    if total == 0 {
        out.push("no stake bonded".to_string());
        return Ok(out);
    }
    for (operator, stake) in per_operator {
        let bps = stake.saturating_mul(10_000) / total;
        if bps > MAX_SHARE_BPS {
            out.push(format!(
                "operator {}… holds {}.{:02}% of stake, cap is {}%",
                &operator[..operator.len().min(12)],
                bps / 100,
                bps % 100,
                MAX_SHARE_BPS / 100
            ));
        }
    }
    Ok(out)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .ok_or("usage: cargo xtask genesis-check <genesis.json>")?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock is before 1970: {e}"))?
        .as_secs();
    let found = problems(&text, now)?;
    if found.is_empty() {
        println!("genesis-check: PASS ({path})");
        return Ok(());
    }
    Err(format!(
        "genesis-check: FAIL ({path})\n  {}",
        found.join("\n  ")
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const NOW: u64 = 1_794_000_000;
    const DAY: u64 = 86_400;
    const TAIL: &str = "23956194387405886379192334";
    const INITIAL: &str = "299452429842573579739904175";
    /// The node's `lifetime_emission` of the mainnet schedule at 3,600-block
    /// epochs (`deploy/genesis/mainnet/README.md`).
    const RESERVE: &str = "122753829695359067419575633036805965021648";

    fn check(text: &str) -> Result<Vec<String>, String> {
        problems(text, NOW)
    }

    fn mining() -> String {
        format!(
            "\"mining\":{{\"initial_epoch_reward\":\"{INITIAL}\",\"halving_epochs\":35064,\"tail_epoch_reward\":\"{TAIL}\"}},"
        )
    }

    fn allocation(address: &str, balance: &str) -> String {
        format!("{{\"address\":\"{address}\",\"balance\":\"{balance}\"}}")
    }

    fn no_mining(g: &str) -> String {
        g.replace(&mining(), "")
    }

    /// The minimum self-bond every fixture bonds: mainnet bonds are exactly it.
    const FLOOR: &str = "1000000000000000000000000000";

    /// Each seat bonds the floor; the value in `bonds` only names the operator's
    /// seats, since a bond above the floor is refused on its own.
    fn genesis(difficulty: u32, weighted: bool, bonds: &[(&str, u128)]) -> String {
        let validators = vec!["\"k\""; bonds.len()].join(",");
        let bonds = bonds
            .iter()
            .map(|(o, _)| format!("{{\"operator\":\"{o}\",\"bond\":\"{FLOOR}\"}}"))
            .collect::<Vec<_>>()
            .join(",");
        let reserve = allocation(&mining_reserve_address(), RESERVE);
        let mining = mining();
        format!(
            "{{\"difficulty_bits\":{difficulty},\"shielded_activation_height\":18446744073709551615,\"timestamp\":{NOW},\"allocations\":[{reserve}],\"bft\":{{\"validators\":[{validators}],\
             {mining}\
             \"fees\":{{\"min_base_fee\":\"1000000\"}},\
             \"staking\":{{\"stake_weighted\":{weighted},\"epoch_blocks\":3600,\
             \"min_self_bond\":\"{FLOOR}\",\
             \"min_delegation\":\"1000000000000000000000000\",\"bonds\":[{bonds}]}}}}}}"
        )
    }

    const EVEN: [(&str, u128); 7] = [
        ("aaaaaaaaaaaaaaaa", 100),
        ("bbbbbbbbbbbbbbbb", 100),
        ("cccccccccccccccc", 100),
        ("dddddddddddddddd", 100),
        ("eeeeeeeeeeeeeeee", 100),
        ("ffffffffffffffff", 100),
        ("1111111111111111", 100),
    ];

    #[test]
    fn seven_even_operators_pass() {
        assert!(check(&genesis(20, true, &EVEN)).unwrap().is_empty());
    }

    #[test]
    fn an_anchor_timeout_below_the_floor_is_refused_and_the_default_passes() {
        let with = |t: u64| {
            genesis(20, true, &EVEN).replace(
                "\"bft\":{",
                &format!("\"bft\":{{\"anchor_timeout_ms\":{t},"),
            )
        };
        for refused in [0, 1, MIN_ANCHOR_TIMEOUT_MS - 1] {
            let found = check(&with(refused)).unwrap();
            assert!(
                found.iter().any(|p| p.contains("anchor_timeout_ms")),
                "{refused}: {found:?}"
            );
        }
        for accepted in [MIN_ANCHOR_TIMEOUT_MS, 1_000] {
            assert!(check(&with(accepted)).unwrap().is_empty(), "{accepted}");
        }
        // Absent means the node's default, 1000 ms.
        assert!(check(&genesis(20, true, &EVEN)).unwrap().is_empty());
    }

    #[test]
    fn a_round_interval_off_one_block_per_second_is_refused_and_the_default_passes() {
        let with = |ms: u64| {
            genesis(20, true, &EVEN).replace(
                "\"bft\":{",
                &format!("\"bft\":{{\"round_interval_ms\":{ms},"),
            )
        };
        for refused in [0, 499, 501, 1_000, 15_000] {
            let found = check(&with(refused)).unwrap();
            assert!(
                found.iter().any(|p| p.contains("round_interval_ms")),
                "{refused}: {found:?}"
            );
        }
        assert!(check(&with(CANONICAL_ROUND_INTERVAL_MS)).unwrap().is_empty());
        // Absent means the node's default, which is the canonical rate.
        assert!(check(&genesis(20, true, &EVEN)).unwrap().is_empty());
    }

    #[test]
    fn one_operator_over_the_cap_fails_even_split_across_bonds() {
        let mut bonds = EVEN.to_vec();
        bonds[0].1 = 100;
        bonds.push(("aaaaaaaaaaaaaaaa", 300)); // same operator, second validator
        let found = check(&genesis(20, true, &bonds)).unwrap();
        assert!(found.iter().any(|p| p.contains("cap is 20%")), "{found:?}");
    }

    #[test]
    fn the_hybrid_testnet_shape_fails_every_rule() {
        let g = genesis(
            0,
            false,
            &[("o", 1_000), ("p", 100), ("p", 100), ("p", 100)],
        );
        let found = check(&g).unwrap();
        // Fewer than seven seats, not stake-weighted, one operator over the cap.
        assert!(found.len() >= 3, "{found:?}");
    }

    #[test]
    fn a_genesis_with_the_shielded_pool_on_fails() {
        let on = genesis(20, true, &EVEN)
            .replace("\"shielded_activation_height\":18446744073709551615,", "");
        let found = check(&on).unwrap();
        assert!(
            found.iter().any(|p| p.contains("shielded pool is not off")),
            "{found:?}"
        );
        let zero = genesis(20, true, &EVEN).replace("18446744073709551615", "0");
        assert!(!check(&zero).unwrap().is_empty());
    }

    #[test]
    fn a_non_genesis_file_is_an_error_not_a_pass() {
        assert!(check("{}").is_err());
    }

    #[test]
    fn a_bft_chain_with_mining_needs_no_block_difficulty() {
        assert!(check(&genesis(0, true, &EVEN)).unwrap().is_empty());
        let found = check(&no_mining(&genesis(0, true, &EVEN))).unwrap();
        assert!(
            found.iter().any(|p| p.contains("proof of work is free")),
            "{found:?}"
        );
    }

    #[test]
    fn a_fee_floor_below_a_million_units_fails() {
        let cheap = genesis(20, true, &EVEN).replace("\"1000000\"", "1");
        let found = check(&cheap).unwrap();
        assert!(found.iter().any(|p| p.contains("S-007")), "{found:?}");
        let free = genesis(20, true, &EVEN).replace("\"fees\":{\"min_base_fee\":\"1000000\"},", "");
        let found = check(&free).unwrap();
        assert!(found.iter().any(|p| p.contains("no bft.fees")), "{found:?}");
    }

    #[test]
    fn dust_or_unset_staking_floors_fail() {
        let dust = genesis(20, true, &EVEN).replace(
            "\"min_self_bond\":\"1000000000000000000000000000\"",
            "\"min_self_bond\":\"1000\"",
        );
        let found = check(&dust).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains("min_self_bond 1000 is below")),
            "{found:?}"
        );
        let unset = genesis(20, true, &EVEN)
            .replace("\"min_delegation\":\"1000000000000000000000000\",", "");
        let found = check(&unset).unwrap();
        assert!(
            found.iter().any(|p| p.contains("min_delegation is unset")),
            "{found:?}"
        );
    }

    #[test]
    fn a_bond_above_the_minimum_is_refused_and_named() {
        let over = genesis(20, true, &EVEN).replacen(
            &format!("\"bond\":\"{FLOOR}\""),
            "\"bond\":\"2000000000000000000000000000\"",
            1,
        );
        let found = check(&over).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains("exactly the minimum") && p.contains("aaaaaaaaaaaa")),
            "{found:?}"
        );
    }

    fn with_allocation(address: &str, balance: &str) -> String {
        genesis(20, true, &EVEN).replace(
            &allocation(&mining_reserve_address(), RESERVE),
            &allocation(address, balance),
        )
    }

    #[test]
    fn a_premine_to_anyone_but_the_reserve_is_refused_and_named() {
        let to = "11".repeat(32);
        let found = check(&with_allocation(&to, "5")).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains(&to) && p.contains("no premine")),
            "{found:?}"
        );
    }

    #[test]
    fn the_reserve_must_hold_exactly_the_lifetime_emission() {
        let reserve = mining_reserve_address();
        for wrong in [
            "122753829695359067419575633036805965021647",
            "122753829695359067419575633036805965021649",
            "0",
        ] {
            let found = check(&with_allocation(&reserve, wrong)).unwrap();
            assert!(
                found.iter().any(|p| p.contains("lifetime emission")),
                "{wrong}: {found:?}"
            );
        }
        // Without the reserve at all, the schedule is unfunded.
        let missing =
            genesis(20, true, &EVEN).replace(&format!("[{}]", allocation(&reserve, RESERVE)), "[]");
        let found = check(&missing).unwrap();
        assert!(found.iter().any(|p| p.contains("holds 0")), "{found:?}");
        // A schedule it cannot price is a problem, not a pass.
        let unpriced = genesis(20, true, &EVEN).replace("\"epoch_blocks\":3600,", "");
        let found = check(&unpriced).unwrap();
        assert!(
            found.iter().any(|p| p.contains("epoch_blocks")),
            "{found:?}"
        );
    }

    #[test]
    fn the_lifetime_emission_is_the_nodes_mainnet_figure() {
        // The node's `MiningRecord::lifetime_emission` for mining.json at
        // 3,600-block epochs, as the mainnet README and `proving.rs` record it.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../deploy/genesis/mainnet/mining.json"
        );
        let mining: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let lifetime = lifetime_emission(
            amount_of(&mining["initial_epoch_reward"]).unwrap(),
            amount_of(&mining["tail_epoch_reward"]).unwrap(),
            mining["halving_epochs"].as_u64().unwrap(),
            3_600,
        )
        .unwrap();
        assert_eq!(lifetime.to_string(), RESERVE);
        // The rehearsal's helper reads the same file to the same figure.
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(mining_reserve(&text, 3_600).unwrap(), RESERVE);
    }

    #[test]
    fn the_mining_reserve_alone_is_allowed_with_or_without_a_0x_prefix() {
        let reserve = mining_reserve_address();
        let amount = RESERVE;
        assert!(
            check(&with_allocation(&reserve, amount))
                .unwrap()
                .is_empty()
        );
        let prefixed = format!("0x{}", reserve.to_uppercase());
        assert!(
            check(&with_allocation(&prefixed, amount))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_reserve_tag_is_the_node_constant() {
        // The bytes of `MINING_RESERVE` in crates/node/src/state/mining.rs.
        let node: [u8; 32] = [
            0x6d, 0x61, 0x79, 0x61, 0x32, 0x63, 0x2f, 0x6d, 0x69, 0x6e, 0x69, 0x6e, 0x67, 0x2f,
            0x72, 0x65, 0x73, 0x65, 0x72, 0x76, 0x65, 0x2f, 0x76, 0x31, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(mining_reserve_address(), hex::encode(node));
    }

    #[test]
    fn a_scheduled_protocol_upgrade_is_refused() {
        let g = genesis(20, true, &EVEN).replace(
            "\"bft\"",
            "\"protocol_upgrades\":[{\"activation_height\":5}],\"bft\"",
        );
        let found = check(&g).unwrap();
        assert!(
            found.iter().any(|p| p.contains("protocol upgrade")),
            "{found:?}"
        );
    }

    #[test]
    fn a_timestamp_is_judged_against_the_clock_passed_in() {
        let at = |ts: u64| {
            genesis(20, true, &EVEN).replace(
                &format!("\"timestamp\":{NOW}"),
                &format!("\"timestamp\":{ts}"),
            )
        };
        assert!(problems(&at(NOW), NOW).unwrap().is_empty());
        assert!(problems(&at(NOW - 364 * DAY), NOW).unwrap().is_empty());
        assert!(problems(&at(NOW + 89 * DAY), NOW).unwrap().is_empty());
        let stale = problems(&at(NOW - 366 * DAY), NOW).unwrap();
        assert!(
            stale.iter().any(|p| p.contains("stale genesis")),
            "{stale:?}"
        );
        let future = problems(&at(NOW + 91 * DAY), NOW).unwrap();
        assert!(
            future.iter().any(|p| p.contains("after the check")),
            "{future:?}"
        );
    }

    #[test]
    fn a_zero_or_missing_tail_reward_is_refused() {
        let zero = genesis(20, true, &EVEN).replace(&format!("\"{TAIL}\""), "\"0\"");
        let found = check(&zero).unwrap();
        assert!(
            found.iter().any(|p| p.contains("tail_epoch_reward")),
            "{found:?}"
        );
        let none = check(&no_mining(&genesis(20, true, &EVEN))).unwrap();
        assert!(
            none.iter().any(|p| p.contains("tail_epoch_reward")),
            "{none:?}"
        );
    }

    #[test]
    fn the_tail_in_the_checked_in_mainnet_mining_json_is_the_one_tested() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../deploy/genesis/mainnet/mining.json"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let mining: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(mining["tail_epoch_reward"], TAIL);
    }
}
