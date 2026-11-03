//! `cargo xtask hybrid-genesis SRC OUT` — the five-validator genesis of the
//! hybrid testnet, cut from the eight-validator one (ADR-057).
//!
//! Keeps validators 0, 4, 5, 6 and 7 and drops 1, 2 and 3. A dropped bond
//! returns to its operator, so total supply is unchanged; the transform checks
//! that before it writes anything. Output is Python's `json.dump(indent=2)` form,
//! byte for byte, with no trailing newline.
//!
//! Shortcut: balances and bonds must fit in `u64` (the transform refuses a
//! larger one rather than round it). Upgrade to `u128`/`U256` sums if a genesis
//! ever carries a balance above 2^64 base units.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::pyfmt;

const VALIDATORS_IN: usize = 8;
const KEEP: [usize; 5] = [0, 4, 5, 6, 7];
const DROP: [usize; 3] = [1, 2, 3];
const USAGE: &str = "usage: cargo xtask hybrid-genesis SRC OUT";

pub fn run(args: &[String]) -> Result<(), String> {
    let [src, out] = args else {
        return Err(USAGE.into());
    };
    let text = fs::read_to_string(src).map_err(|e| format!("{src}: {e}"))?;
    let mut genesis: Value = serde_json::from_str(&text).map_err(|e| format!("{src}: {e}"))?;
    let (supply, bonds) = cut(&mut genesis)?;
    let pretty = pyfmt::json(&genesis, 2)?;
    fs::write(out, pretty).map_err(|e| format!("{out}: {e}"))?;

    let total: u64 = bonds.iter().sum();
    let share = bonds.first().map_or(0.0, |first| share_of(*first, total));
    let listed: Vec<String> = bonds.iter().map(u64::to_string).collect();
    println!(
        "supply {supply} bonds [{}] oracle share {}",
        listed.join(", "),
        pyfmt::repr(share),
    );
    Ok(())
}

#[allow(clippy::cast_precision_loss)] // the share is a report figure, not consensus arithmetic
fn share_of(part: u64, total: u64) -> f64 {
    part as f64 / total as f64
}

/// Applies the cut in place. Returns `(supply, bonds kept)`.
fn cut(genesis: &mut Value) -> Result<(u64, Vec<u64>), String> {
    let bft = genesis.get("bft").cloned().ok_or("genesis has no `bft`")?;
    let validators = array(&bft, "validators")?;
    let bonds = array(&bft["staking"], "bonds")?;
    if validators.len() != VALIDATORS_IN || bonds.len() != VALIDATORS_IN {
        return Err(format!(
            "expected {VALIDATORS_IN} validators and bonds, found {} and {}",
            validators.len(),
            bonds.len()
        ));
    }
    let allocations = array(genesis, "allocations")?;

    // A dict keyed by address, as the script built it: a repeated address keeps
    // the first position and the last value.
    let mut alloc: Map<String, Value> = Map::new();
    for entry in &allocations {
        alloc.insert(text(entry, "address")?.to_string(), entry.clone());
    }
    let before = sum_field(&allocations, "balance")?
        .checked_add(sum_field(&bonds, "bond")?)
        .ok_or("supply overflows u64")?;

    for &i in &DROP {
        let operator = text(&bonds[i], "operator")?.to_string();
        let bond = int(&bonds[i], "bond")?;
        let entry = alloc
            .entry(operator.clone())
            .or_insert_with(|| json!({ "address": operator, "balance": 0 }));
        let balance = int(entry, "balance")?;
        let credited = balance.checked_add(bond).ok_or("balance overflows u64")?;
        entry["balance"] = json!(credited);
    }

    let kept_bonds: Vec<Value> = KEEP.iter().map(|&i| bonds[i].clone()).collect();
    let kept_validators: Vec<Value> = KEEP.iter().map(|&i| validators[i].clone()).collect();
    let balances: Vec<Value> = alloc.values().cloned().collect();
    let after = sum_field(&balances, "balance")?
        .checked_add(sum_field(&kept_bonds, "bond")?)
        .ok_or("supply overflows u64")?;
    if before != after {
        return Err(format!(
            "supply changed in the cut: {before} before, {after} after"
        ));
    }

    let bft_mut = genesis.get_mut("bft").ok_or("genesis has no `bft`")?;
    bft_mut["validators"] = Value::Array(kept_validators);
    bft_mut["staking"]["bonds"] = Value::Array(kept_bonds.clone());
    genesis["allocations"] = Value::Array(alloc.into_iter().map(|(_, v)| v).collect());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    if let Some(root) = genesis.as_object_mut() {
        root.insert("timestamp".into(), json!(now.as_secs()));
    }

    let kept = kept_bonds
        .iter()
        .map(|b| int(b, "bond"))
        .collect::<Result<Vec<u64>, _>>()?;
    Ok((after, kept))
}

fn array(value: &Value, key: &str) -> Result<Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| format!("`{key}` is missing or not an array"))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` is missing or not a string"))
}

fn int(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("`{key}` is missing or not a u64"))
}

fn sum_field(items: &[Value], key: &str) -> Result<u64, String> {
    items.iter().try_fold(0u64, |acc, item| {
        acc.checked_add(int(item, key)?)
            .ok_or_else(|| "sum overflows u64".to_string())
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::cut;
    use serde_json::{Value, json};

    /// Eight validators with bond `10 * (i + 1)`, one allocation each, and a
    /// dropped operator that already holds an allocation (so it is credited).
    fn eight() -> Value {
        let validators: Vec<Value> = (0..8).map(|i| json!({ "id": i })).collect();
        let bonds: Vec<Value> = (0..8u64)
            .map(|i| json!({ "operator": format!("op{i}"), "bond": 10 * (i + 1) }))
            .collect();
        let allocations: Vec<Value> = (0..8)
            .map(|i| json!({ "address": format!("op{i}"), "balance": 1 }))
            .collect();
        json!({
            "bft": { "validators": validators, "staking": { "bonds": bonds } },
            "allocations": allocations,
        })
    }

    #[test]
    fn the_cut_keeps_five_and_preserves_supply() {
        let mut g = eight();
        let (supply, bonds) = cut(&mut g).expect("a valid eight-validator genesis");
        // Before: 8 balances of 1 plus bonds 10+20+...+80 = 360, total 368.
        assert_eq!(supply, 368);
        assert_eq!(bonds, vec![10, 50, 60, 70, 80]);
        assert_eq!(g["bft"]["validators"].as_array().map(Vec::len), Some(5));
        // Dropped operators op1..op3 are still on the books, with their bonds.
        let credited = g["allocations"]
            .as_array()
            .expect("allocations")
            .iter()
            .find(|a| a["address"] == "op1")
            .expect("op1 kept");
        assert_eq!(credited["balance"], json!(21));
    }

    #[test]
    fn a_genesis_that_is_not_eight_validators_is_refused() {
        let mut g = eight();
        g["bft"]["validators"].as_array_mut().expect("array").pop();
        assert!(cut(&mut g).is_err());
    }
}
