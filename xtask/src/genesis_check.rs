//! `cargo xtask genesis-check <genesis.json>` — mainnet genesis policy, from the file.
//!
//! Decided 2026-10-25: no operator holds more than 20% of the bonded stake,
//! at least 7 validators, stake-weighted voting, and a real proof-of-work
//! difficulty. Concentration is counted per **operator**, not per bond: one
//! operator running four validators is one share (the hybrid testnet is
//! exactly that case, and fails here by design). Hosts cannot be read from a
//! genesis file, so "3 or more hosts" stays a human check in `go-no-go`.

use serde::Deserialize;
use std::collections::BTreeMap;

const MAX_SHARE_BPS: u128 = 2_000;
const MIN_VALIDATORS: usize = 7;

#[derive(Deserialize)]
struct Genesis {
    difficulty_bits: u32,
    bft: Bft,
}

#[derive(Deserialize)]
struct Bft {
    validators: Vec<String>,
    staking: Staking,
}

#[derive(Deserialize)]
struct Staking {
    #[serde(default)]
    stake_weighted: bool,
    bonds: Vec<Bond>,
}

#[derive(Deserialize)]
struct Bond {
    operator: String,
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
    if !g.bft.staking.stake_weighted {
        out.push("voting is not stake-weighted".to_string());
    }
    if g.difficulty_bits == 0 {
        out.push("difficulty_bits is 0: proof of work is free".to_string());
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
             \"staking\":{{\"stake_weighted\":{weighted},\"bonds\":[{bonds}]}}}}}}"
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
}
