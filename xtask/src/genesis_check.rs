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
const MIN_SELF_BOND: u128 = 1_000 * COIN;
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

#[derive(Deserialize)]
struct Bft {
    validators: Vec<String>,
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
fn problems(text: &str, now: u64) -> Result<Vec<String>, String> {
    let g: Genesis = serde_json::from_str(text).map_err(|e| format!("not a genesis file: {e}"))?;
    let mut out = Vec::new();
    let reserve = mining_reserve_address();
    for a in &g.allocations {
        if normalized_address(&a.address) != reserve {
            out.push(format!(
                "allocation to {} of {} base units: only the keyless mining reserve may be funded at genesis (zero premine, ADR-044)",
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
    if g.bft.validators.len() < MIN_VALIDATORS {
        out.push(format!(
            "{} validators, need at least {MIN_VALIDATORS}",
            g.bft.validators.len()
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

    fn check(text: &str) -> Result<Vec<String>, String> {
        problems(text, NOW)
    }

    fn no_mining(g: String) -> String {
        g.replace(
            &format!("\"mining\":{{\"tail_epoch_reward\":\"{TAIL}\"}},"),
            "",
        )
    }

    fn genesis(difficulty: u32, weighted: bool, bonds: &[(&str, u128)]) -> String {
        let validators = vec!["\"k\""; bonds.len()].join(",");
        let bonds = bonds
            .iter()
            .map(|(o, b)| format!("{{\"operator\":\"{o}\",\"bond\":{b}}}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"difficulty_bits\":{difficulty},\"shielded_activation_height\":18446744073709551615,\"timestamp\":{NOW},\"bft\":{{\"validators\":[{validators}],\
             \"mining\":{{\"tail_epoch_reward\":\"{TAIL}\"}},\
             \"fees\":{{\"min_base_fee\":\"1000000\"}},\
             \"staking\":{{\"stake_weighted\":{weighted},\
             \"min_self_bond\":\"1000000000000000000000000000\",\
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
        assert!(found.len() >= 4, "{found:?}");
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
        let found = check(&no_mining(genesis(0, true, &EVEN))).unwrap();
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
    fn a_bond_written_as_a_decimal_string_is_read() {
        let g = genesis(20, true, &EVEN).replace("\"bond\":100", "\"bond\":\"100\"");
        assert!(check(&g).unwrap().is_empty());
    }

    fn with_allocation(address: &str, balance: &str) -> String {
        genesis(20, true, &EVEN).replace(
            "\"bft\"",
            &format!(
                "\"allocations\":[{{\"address\":\"{address}\",\"balance\":\"{balance}\"}}],\"bft\""
            ),
        )
    }

    #[test]
    fn a_premine_to_anyone_but_the_reserve_is_refused_and_named() {
        let to = "11".repeat(32);
        let found = check(&with_allocation(&to, "5")).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains(&to) && p.contains("zero premine")),
            "{found:?}"
        );
    }

    #[test]
    fn the_mining_reserve_alone_is_allowed_with_or_without_a_0x_prefix() {
        let reserve = mining_reserve_address();
        let amount = "122753829695359067419575633036805965021648";
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
        let none = check(&no_mining(genesis(20, true, &EVEN))).unwrap();
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
