//! `cargo xtask hybrid-genesis SRC OUT` — the five-validator genesis of the
//! hybrid testnet, cut from the eight-validator one (ADR-057).
//!
//! Keeps validators 0, 4, 5, 6 and 7 and drops 1, 2 and 3. A dropped bond
//! returns to its operator, so total supply is unchanged; the transform checks
//! that before it writes anything. Output is Python's `json.dump(indent=2)` form,
//! byte for byte, with no trailing newline, and key order is the input's.
//!
//! Shortcut: balances and bonds must fit in `u64` (the transform refuses a
//! larger one rather than round it). Upgrade to `u128`/`U256` sums if a genesis
//! ever carries a balance above 2^64 base units.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ordered_json::Json;
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
    let mut genesis = Json::parse(&text).map_err(|e| format!("{src}: {e}"))?;
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
fn cut(genesis: &mut Json) -> Result<(u64, Vec<u64>), String> {
    let bft = field(genesis, "bft")?.clone();
    let validators = array(&bft, "validators")?;
    let bonds = array(field(&bft, "staking")?, "bonds")?;
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
    let mut alloc = Json::Object(Vec::new());
    for entry in &allocations {
        alloc.set(text(entry, "address")?, entry.clone())?;
    }
    let before = sum_field(&allocations, "balance")?
        .checked_add(sum_field(&bonds, "bond")?)
        .ok_or("supply overflows u64")?;

    for &i in &DROP {
        let operator = text(&bonds[i], "operator")?.to_string();
        let bond = int(&bonds[i], "bond")?;
        if alloc.get(&operator).is_none() {
            let fresh = Json::Object(vec![
                ("address".into(), Json::String(operator.clone())),
                ("balance".into(), Json::uint(0)),
            ]);
            alloc.set(&operator, fresh)?;
        }
        let entry = alloc.get_mut(&operator).ok_or("allocation vanished")?;
        let credited = int(entry, "balance")?
            .checked_add(bond)
            .ok_or("balance overflows u64")?;
        entry.set("balance", Json::uint(credited))?;
    }

    let kept_bonds: Vec<Json> = KEEP.iter().map(|&i| bonds[i].clone()).collect();
    let kept_validators: Vec<Json> = KEEP.iter().map(|&i| validators[i].clone()).collect();
    let balances = values(&alloc);
    let after = sum_field(&balances, "balance")?
        .checked_add(sum_field(&kept_bonds, "bond")?)
        .ok_or("supply overflows u64")?;
    if before != after {
        return Err(format!(
            "supply changed in the cut: {before} before, {after} after"
        ));
    }

    let staking = bft_staking_mut(genesis)?;
    staking.set("bonds", Json::Array(kept_bonds.clone()))?;
    bft_mut(genesis)?.set("validators", Json::Array(kept_validators))?;
    genesis.set("allocations", Json::Array(balances))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    genesis.set("timestamp", Json::uint(now.as_secs()))?;

    let kept = kept_bonds
        .iter()
        .map(|b| int(b, "bond"))
        .collect::<Result<Vec<u64>, _>>()?;
    Ok((after, kept))
}

fn bft_mut(genesis: &mut Json) -> Result<&mut Json, String> {
    genesis
        .get_mut("bft")
        .ok_or_else(|| "genesis has no `bft`".into())
}

fn bft_staking_mut(genesis: &mut Json) -> Result<&mut Json, String> {
    bft_mut(genesis)?
        .get_mut("staking")
        .ok_or_else(|| "`bft` has no `staking`".into())
}

/// The allocations in the dict's order, one entry per address.
fn values(alloc: &Json) -> Vec<Json> {
    match alloc {
        Json::Object(entries) => entries.iter().map(|(_, v)| v.clone()).collect(),
        _ => Vec::new(),
    }
}

fn field<'a>(value: &'a Json, key: &str) -> Result<&'a Json, String> {
    value.get(key).ok_or_else(|| format!("`{key}` is missing"))
}

fn array(value: &Json, key: &str) -> Result<Vec<Json>, String> {
    field(value, key)?
        .as_array()
        .cloned()
        .ok_or_else(|| format!("`{key}` is not an array"))
}

fn text<'a>(value: &'a Json, key: &str) -> Result<&'a str, String> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("`{key}` is missing or not a string"))
}

fn int(value: &Json, key: &str) -> Result<u64, String> {
    field(value, key)?
        .as_u64()
        .ok_or_else(|| format!("`{key}` is missing or not a u64"))
}

fn sum_field(items: &[Json], key: &str) -> Result<u64, String> {
    items.iter().try_fold(0u64, |acc, item| {
        acc.checked_add(int(item, key)?)
            .ok_or_else(|| "sum overflows u64".to_string())
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::cut;
    use crate::ordered_json::Json;

    /// Eight validators with bond `10 * (i + 1)`, one allocation each, and a
    /// dropped operator that already holds an allocation (so it is credited).
    fn eight() -> Json {
        let validators: Vec<String> = (0..8).map(|i| format!(r#"{{"id": {i}}}"#)).collect();
        let bonds: Vec<String> = (0..8u64)
            .map(|i| format!(r#"{{"operator": "op{i}", "bond": {}}}"#, 10 * (i + 1)))
            .collect();
        let allocations: Vec<String> = (0..8)
            .map(|i| format!(r#"{{"address": "op{i}", "balance": 1}}"#))
            .collect();
        let text = format!(
            r#"{{"bft": {{"validators": [{}], "staking": {{"bonds": [{}]}}}}, "allocations": [{}]}}"#,
            validators.join(", "),
            bonds.join(", "),
            allocations.join(", ")
        );
        Json::parse(&text).expect("valid fixture")
    }

    fn balance_of(genesis: &Json, address: &str) -> Option<u64> {
        genesis
            .get("allocations")?
            .as_array()?
            .iter()
            .find_map(|a| {
                (a.get("address")?.as_str()? == address).then(|| a.get("balance")?.as_u64())?
            })
    }

    #[test]
    fn the_cut_keeps_five_and_preserves_supply() {
        let mut g = eight();
        let (supply, bonds) = cut(&mut g).expect("a valid eight-validator genesis");
        // Before: 8 balances of 1 plus bonds 10+20+...+80 = 360, total 368.
        assert_eq!(supply, 368);
        assert_eq!(bonds, vec![10, 50, 60, 70, 80]);
        assert_eq!(
            g.get("bft")
                .and_then(|b| b.get("validators"))
                .and_then(Json::as_array)
                .map(Vec::len),
            Some(5)
        );
        // Dropped operators op1..op3 are still on the books, credited with their bonds.
        assert_eq!(balance_of(&g, "op1"), Some(21));
    }

    #[test]
    fn key_order_of_the_input_is_kept() {
        let mut g = eight();
        cut(&mut g).expect("valid");
        let out = crate::pyfmt::json(&g, 2).expect("serialises");
        let bft = out.find("\"bft\"").expect("bft");
        let allocations = out.find("\"allocations\"").expect("allocations");
        assert!(
            bft < allocations,
            "bft precedes allocations as in the input"
        );
    }

    #[test]
    fn a_genesis_that_is_not_eight_validators_is_refused() {
        let mut g = eight();
        if let Some(Json::Array(items)) = g.get_mut("bft").and_then(|b| b.get_mut("validators")) {
            items.pop();
        }
        assert!(cut(&mut g).is_err());
    }
}
