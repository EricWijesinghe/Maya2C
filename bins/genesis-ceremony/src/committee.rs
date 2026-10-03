//! The DAG-BFT committee a launch genesis names (gate 5).
//!
//! Each validator operator runs `maya2c-node --generate-validator-key <PATH>`
//! on their own machine and sends back only the public key it prints; the
//! secret never leaves that machine. The coordinator lists the keys, one per
//! line, in committee order:
//!
//! ```text
//! # label   validator public key (hex)    [operator address  bond]
//! alice     6db2…de                         c1ef…a8            1000000000
//! bob       9a41…07                         77d0…3c            1000000000
//! ```
//!
//! Operator and bond are all-or-nothing: either every line carries them and
//! the chain stakes from genesis (ADR-028), or none does and the genesis
//! committee orders the chain for ever.

use std::error::Error;
use std::path::Path;

use custom_l1_node::genesis::{BftGenesis, FeesGenesis, GenesisBond, StakingGenesis};

/// Hex characters in an ML-DSA-65 public key (1,952 bytes).
const VALIDATOR_KEY_HEX: usize = 2 * 1_952;
/// Hex characters in an address.
const ADDRESS_HEX: usize = 64;
/// Fewest validators a launch committee may have: DAG-BFT tolerates `f`
/// faults among `3f + 1`, so below four a single fault halts the chain
/// (mainnet gate 4).
pub const MIN_LAUNCH_VALIDATORS: usize = 4;
/// Blocks per staking epoch unless the coordinator says otherwise; the
/// testnet's value.
pub const DEFAULT_EPOCH_BLOCKS: u64 = 3_600;

/// The fee market the public testnet runs (base fee 1 per byte, a 2.5 MiB
/// target, at most 1/8 change per block), so a launch starts from parameters
/// that have been exercised rather than untested ones.
const LAUNCH_FEES: FeesGenesis = FeesGenesis {
    initial_base_fee: 1,
    min_base_fee: 1,
    target_block_bytes: 2_621_440,
    change_denominator: 8,
};

/// One line of the committee file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The operator's label, for the commitment sheet.
    pub label: String,
    /// The validator's public key, lowercase hex.
    pub key: String,
    /// The operator account and its bond, when the chain stakes.
    pub bond: Option<(String, u64)>,
}

/// Parses a committee file's text.
///
/// # Errors
///
/// A malformed line, a key or address of the wrong length or not hex, a
/// repeated key, bonds on some lines but not others, or an empty file.
pub fn parse(text: &str) -> Result<Vec<Member>, Box<dyn Error>> {
    let mut members: Vec<Member> = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let member = parse_line(line).map_err(|e| format!("line {}: {e}", number + 1))?;
        if members.iter().any(|m| m.key == member.key) {
            return Err(format!("line {}: repeats an earlier validator key", number + 1).into());
        }
        members.push(member);
    }
    if members.is_empty() {
        return Err("the committee file names no validator".into());
    }
    let staked = members.iter().filter(|m| m.bond.is_some()).count();
    if staked != 0 && staked != members.len() {
        return Err("give every validator an operator and bond, or none".into());
    }
    Ok(members)
}

fn parse_line(line: &str) -> Result<Member, Box<dyn Error>> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let (label, key, bond) = match fields.as_slice() {
        [label, key] => (*label, *key, None),
        [label, key, operator, bond] => (*label, *key, Some((*operator, *bond))),
        _ => return Err("expected `label key` or `label key operator bond`".into()),
    };
    let key = hex_field(key, VALIDATOR_KEY_HEX, "validator key")?;
    let bond = match bond {
        Some((operator, amount)) => {
            let amount: u64 = amount
                .parse()
                .map_err(|_| format!("bond '{amount}' is not a number"))?;
            if amount == 0 {
                return Err("a bond must be non-zero".into());
            }
            Some((
                hex_field(operator, ADDRESS_HEX, "operator address")?,
                amount,
            ))
        }
        None => None,
    };
    Ok(Member {
        label: label.to_owned(),
        key,
        bond,
    })
}

fn hex_field(value: &str, len: usize, what: &str) -> Result<String, Box<dyn Error>> {
    let value = value
        .strip_prefix("0x")
        .unwrap_or(value)
        .to_ascii_lowercase();
    if value.len() != len || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{what} must be {len} hex characters").into());
    }
    Ok(value)
}

/// Reads a committee file.
///
/// # Errors
///
/// The file cannot be read, or [`parse`] refuses it.
pub fn read(path: &Path) -> Result<Vec<Member>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()).into())
}

/// The genesis committee for `members`, with the testnet's fee market and,
/// when the members carry bonds, staking over `epoch_blocks`.
#[must_use]
pub fn genesis(members: &[Member], epoch_blocks: u64) -> BftGenesis {
    let mut bft = BftGenesis::with_validators(members.iter().map(|m| m.key.clone()).collect());
    bft.fees = Some(LAUNCH_FEES);
    let bonds: Vec<GenesisBond> = members
        .iter()
        .filter_map(|m| m.bond.clone())
        .map(|(operator, bond)| GenesisBond {
            operator,
            bond,
            commission_bps: 0,
        })
        .collect();
    if !bonds.is_empty() {
        bft.staking = Some(StakingGenesis::with_bonds(epoch_blocks, bonds));
    }
    bft
}

/// Refuses a value-bearing chain whose committee is too small to survive one
/// faulty validator (mainnet gate 4).
///
/// # Errors
///
/// Fewer than [`MIN_LAUNCH_VALIDATORS`] members on a value-bearing chain.
pub fn check_launch_size(value_bearing: bool, members: &[Member]) -> Result<(), Box<dyn Error>> {
    if value_bearing && members.len() < MIN_LAUNCH_VALIDATORS {
        return Err(format!(
            "a value-bearing chain needs at least {MIN_LAUNCH_VALIDATORS} validators run by \
             separate operators (gate 4); this committee has {}",
            members.len()
        )
        .into());
    }
    Ok(())
}

/// The commitment sheet's committee section.
#[must_use]
pub fn sheet_lines(members: &[Member]) -> String {
    members
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let fingerprint = &m.key[..16];
            match &m.bond {
                Some((operator, bond)) => format!(
                    "  {i:>2}  {:<16} {fingerprint}…  operator {}…  bond {bond}",
                    m.label,
                    &operator[..12]
                ),
                None => format!("  {i:>2}  {:<16} {fingerprint}…", m.label),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: char) -> String {
        std::iter::repeat_n(byte, VALIDATOR_KEY_HEX).collect()
    }

    fn address(byte: char) -> String {
        std::iter::repeat_n(byte, ADDRESS_HEX).collect()
    }

    #[test]
    fn a_committee_file_parses_with_comments_and_blank_lines() {
        let text = format!(
            "# launch committee\n\nalice {}\nbob {}  # second\n",
            key('a'),
            key('b')
        );
        let members = parse(&text).expect("parse");
        assert_eq!(members.len(), 2);
        assert_eq!(members[1].label, "bob");
        let bft = genesis(&members, DEFAULT_EPOCH_BLOCKS);
        assert_eq!(bft.validators, vec![key('a'), key('b')]);
        assert!(bft.staking.is_none());
        assert_eq!(bft.fees, Some(LAUNCH_FEES));
    }

    #[test]
    fn bonds_turn_on_staking_in_committee_order() {
        let text = format!(
            "a {} {} 10\nb {} {} 20\n",
            key('a'),
            address('1'),
            key('b'),
            address('2')
        );
        let bft = genesis(&parse(&text).expect("parse"), 7);
        let staking = bft.staking.expect("staked");
        assert_eq!(staking.epoch_blocks, 7);
        assert_eq!(staking.bonds[1].operator, address('2'));
        assert_eq!(staking.bonds[1].bond, 20);
    }

    #[test]
    fn malformed_committees_are_refused() {
        let cases = [
            String::new(),
            "alice abc".to_owned(),
            format!("a {}\nb {}", key('a'), key('a')),
            format!("a {} {} 10\nb {}", key('a'), address('1'), key('b')),
            format!("a {} {} 0", key('a'), address('1')),
            format!("a {} {} ten", key('a'), address('1')),
            format!("a {} xyz 10", key('a')),
        ];
        for case in &cases {
            assert!(
                parse(case).is_err(),
                "accepted: {}",
                &case[..case.len().min(40)]
            );
        }
    }

    #[test]
    fn mainnet_needs_four_validators_a_testnet_does_not() {
        let three: Vec<Member> = ['a', 'b', 'c']
            .iter()
            .map(|c| Member {
                label: c.to_string(),
                key: key(*c),
                bond: None,
            })
            .collect();
        assert!(check_launch_size(true, &three).is_err());
        assert!(check_launch_size(false, &three).is_ok());
        let mut four = three.clone();
        four.push(Member {
            label: "d".into(),
            key: key('d'),
            bond: None,
        });
        assert!(check_launch_size(true, &four).is_ok());
    }
}
