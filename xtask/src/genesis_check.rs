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
/// and 1 coin per delegation. Proposed 2026-10-30 and awaiting Eric's answer;
/// a different answer changes these two lines. The devnet defaults (1,000 and
/// 10 base units) are dust: a free validator seat, a free delegation.
const MIN_SELF_BOND: u128 = 1_000 * COIN;
const MIN_DELEGATION: u128 = COIN;
const COIN: u128 = 1_000_000_000_000_000_000_000_000;

#[derive(Deserialize)]
struct Genesis {
    difficulty_bits: u32,
    bft: Bft,
}

#[derive(Deserialize)]
struct Bft {
    validators: Vec<String>,
    staking: Staking,
    #[serde(default)]
    fees: Option<Fees>,
    #[serde(default)]
    mining: Option<serde_json::Value>,
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

/// Every policy breach in `text`; empty means the genesis passes.
fn problems(text: &str) -> Result<Vec<String>, String> {
    let g: Genesis = serde_json::from_str(text).map_err(|e| format!("not a genesis file: {e}"))?;
    let mut out = Vec::new();
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
    let found = problems(&text)?;
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

    fn genesis(difficulty: u32, weighted: bool, bonds: &[(&str, u128)]) -> String {
        let validators = vec!["\"k\""; bonds.len()].join(",");
        let bonds = bonds
            .iter()
            .map(|(o, b)| format!("{{\"operator\":\"{o}\",\"bond\":{b}}}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"difficulty_bits\":{difficulty},\"bft\":{{\"validators\":[{validators}],\
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
        assert!(problems(&genesis(20, true, &EVEN)).unwrap().is_empty());
    }

    #[test]
    fn one_operator_over_the_cap_fails_even_split_across_bonds() {
        let mut bonds = EVEN.to_vec();
        bonds[0].1 = 100;
        bonds.push(("aaaaaaaaaaaaaaaa", 300)); // same operator, second validator
        let found = problems(&genesis(20, true, &bonds)).unwrap();
        assert!(found.iter().any(|p| p.contains("cap is 20%")), "{found:?}");
    }

    #[test]
    fn the_hybrid_testnet_shape_fails_every_rule() {
        let g = genesis(
            0,
            false,
            &[("o", 1_000), ("p", 100), ("p", 100), ("p", 100)],
        );
        let found = problems(&g).unwrap();
        assert!(found.len() >= 4, "{found:?}");
    }

    #[test]
    fn a_non_genesis_file_is_an_error_not_a_pass() {
        assert!(problems("{}").is_err());
    }

    #[test]
    fn a_bft_chain_with_mining_needs_no_block_difficulty() {
        let mining = genesis(0, true, &EVEN).replace("\"fees\"", "\"mining\":{},\"fees\"");
        assert!(problems(&mining).unwrap().is_empty());
        let found = problems(&genesis(0, true, &EVEN)).unwrap();
        assert!(
            found.iter().any(|p| p.contains("proof of work is free")),
            "{found:?}"
        );
    }

    #[test]
    fn a_fee_floor_below_a_million_units_fails() {
        let cheap = genesis(20, true, &EVEN).replace("\"1000000\"", "1");
        let found = problems(&cheap).unwrap();
        assert!(found.iter().any(|p| p.contains("S-007")), "{found:?}");
        let free = genesis(20, true, &EVEN).replace("\"fees\":{\"min_base_fee\":\"1000000\"},", "");
        let found = problems(&free).unwrap();
        assert!(found.iter().any(|p| p.contains("no bft.fees")), "{found:?}");
    }

    #[test]
    fn dust_or_unset_staking_floors_fail() {
        let dust = genesis(20, true, &EVEN).replace(
            "\"min_self_bond\":\"1000000000000000000000000000\"",
            "\"min_self_bond\":\"1000\"",
        );
        let found = problems(&dust).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains("min_self_bond 1000 is below")),
            "{found:?}"
        );
        let unset = genesis(20, true, &EVEN)
            .replace("\"min_delegation\":\"1000000000000000000000000\",", "");
        let found = problems(&unset).unwrap();
        assert!(
            found.iter().any(|p| p.contains("min_delegation is unset")),
            "{found:?}"
        );
    }

    #[test]
    fn a_bond_written_as_a_decimal_string_is_read() {
        let g = genesis(20, true, &EVEN).replace("\"bond\":100", "\"bond\":\"100\"");
        assert!(problems(&g).unwrap().is_empty());
    }
}
