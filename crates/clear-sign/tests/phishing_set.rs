//! Master Prompt 22 §4: the wallet must warn on every phishing and drainer
//! pattern in the test set, and the rate of false alarms on ordinary
//! transactions is measured.
//!
//! The set is **synthesized**: 11 documented pattern families × 20
//! parameter variations = 220 cases, not 200 captured mainnet transactions.
//! Families: unlimited approval, approve-all (NFT operator), permit
//! signature (unlimited and limited), address poisoning, balance drain,
//! claim/simulation mismatch, hidden approval, malicious upgrade (the Bybit
//! shape), owner/key change, fake airdrop claim. Each case must raise the
//! warning its family is defined by — not merely some warning.

#![allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;

use maya_clear_sign::{Address, Context, Effect, UNLIMITED, Warning, render, review};

fn addr(tag: u8, i: u8) -> Address {
    let mut a = [tag; 32];
    a[15] = i;
    a
}

const NATIVE: Address = [0; 32];

fn context() -> Context {
    let contacts: BTreeSet<Address> = (0..20).map(|i| addr(0xC0, i)).collect();
    let verified: BTreeSet<Address> = (0..20).map(|i| addr(0xE0, i)).collect();
    Context {
        contacts,
        verified,
        balances: vec![(NATIVE, 1_000_000), (addr(0x70, 0), 5_000)],
    }
}

/// A look-alike of contact `i`: same first two and last two bytes, different middle.
fn poison(i: u8) -> Address {
    let mut a = addr(0xC0, i);
    a[10] ^= 0x5A;
    a
}

/// (family, claimed, simulated, the warning the family must raise)
fn malicious(i: u8) -> Vec<(&'static str, Vec<Effect>, Vec<Effect>, Warning)> {
    let token = addr(0x70, 0);
    let thief = addr(0xBA, i);
    let swap = Effect::Call {
        contract: addr(0xE0, i % 20),
        method: "swap".into(),
    };
    vec![
        (
            "unlimited approval",
            vec![Effect::Approve {
                token,
                spender: thief,
                amount: 10,
            }],
            vec![Effect::Approve {
                token,
                spender: thief,
                amount: UNLIMITED + u128::from(i),
            }],
            Warning::UnlimitedApproval,
        ),
        (
            "approve-all",
            vec![Effect::Call {
                contract: thief,
                method: "mint".into(),
            }],
            vec![Effect::ApproveAll {
                collection: addr(0x71, i),
                operator: thief,
            }],
            Warning::ApproveAll,
        ),
        (
            "permit, unlimited",
            vec![Effect::Permit {
                token,
                spender: thief,
                amount: UNLIMITED,
            }],
            vec![Effect::Permit {
                token,
                spender: thief,
                amount: UNLIMITED,
            }],
            Warning::PermitSignature,
        ),
        (
            "permit, limited",
            vec![Effect::Permit {
                token,
                spender: thief,
                amount: 100 + u128::from(i),
            }],
            vec![Effect::Permit {
                token,
                spender: thief,
                amount: 100 + u128::from(i),
            }],
            Warning::PermitSignature,
        ),
        (
            "address poisoning",
            vec![Effect::Transfer {
                token: NATIVE,
                to: poison(i % 20),
                amount: 1_000,
            }],
            vec![Effect::Transfer {
                token: NATIVE,
                to: poison(i % 20),
                amount: 1_000,
            }],
            Warning::LookAlikeRecipient,
        ),
        (
            "balance drain",
            vec![Effect::Transfer {
                token: NATIVE,
                to: thief,
                amount: 900_000 + 5_000 * u128::from(i),
            }],
            vec![Effect::Transfer {
                token: NATIVE,
                to: thief,
                amount: 900_000 + 5_000 * u128::from(i),
            }],
            Warning::DrainsBalance,
        ),
        (
            "claim mismatch",
            vec![Effect::Transfer {
                token: NATIVE,
                to: addr(0xC0, i % 20),
                amount: 10,
            }],
            vec![Effect::Transfer {
                token: NATIVE,
                to: thief,
                amount: 10,
            }],
            Warning::ClaimMismatch,
        ),
        (
            "hidden approval",
            vec![swap.clone()],
            vec![
                swap,
                Effect::Approve {
                    token,
                    spender: addr(0xE0, i % 20),
                    amount: 50,
                },
            ],
            Warning::ClaimMismatch,
        ),
        (
            "malicious upgrade (Bybit shape)",
            vec![Effect::Transfer {
                token: NATIVE,
                to: addr(0xC0, i % 20),
                amount: 0,
            }],
            vec![Effect::Upgrade {
                contract: addr(0x5A, i),
            }],
            Warning::ControlChange,
        ),
        (
            "owner/key change",
            vec![Effect::Call {
                contract: addr(0xE0, i % 20),
                method: "configure".into(),
            }],
            vec![Effect::SetOwner {
                target: addr(0x5B, i),
                owner: thief,
            }],
            Warning::ControlChange,
        ),
    ]
}

fn fake_airdrop(i: u8) -> (&'static str, Vec<Effect>, Vec<Effect>, Warning) {
    let thief = addr(0xBA, i);
    let claim = Effect::Call {
        contract: thief,
        method: "claimAirdrop".into(),
    };
    (
        "fake airdrop",
        vec![claim.clone()],
        vec![
            claim,
            Effect::Transfer {
                token: addr(0x70, 0),
                to: thief,
                amount: 4_800,
            },
        ],
        Warning::ClaimMismatch,
    )
}

#[test]
fn every_one_of_220_drainer_patterns_is_flagged_with_its_familys_warning() {
    let ctx = context();
    let mut cases = Vec::new();
    for i in 0..20u8 {
        cases.extend(malicious(i));
        cases.push(fake_airdrop(i));
    }
    let mut per_family = std::collections::BTreeMap::new();
    let mut missed = Vec::new();
    for (family, claimed, simulated, must) in &cases {
        let w = review(claimed, simulated, &ctx);
        *per_family.entry(*family).or_insert(0) += 1;
        if !w.contains(must) {
            missed.push(format!("{family}: expected {must:?}, got {w:?}"));
        }
    }
    println!(
        "{} malicious cases in {} families: {:?}; missed {}",
        cases.len(),
        per_family.len(),
        per_family,
        missed.len()
    );
    assert!(cases.len() >= 200, "the set must hold at least 200 cases");
    assert!(missed.is_empty(), "{missed:#?}");
}

#[test]
fn ordinary_transactions_are_measured_for_false_alarms() {
    let ctx = context();
    let mut benign = Vec::new();
    for i in 0..25u8 {
        let to = addr(0xC0, i % 20);
        benign.push(vec![Effect::Transfer {
            token: NATIVE,
            to,
            amount: 1_000 + u128::from(i),
        }]);
        benign.push(vec![Effect::Approve {
            token: addr(0x70, 0),
            spender: addr(0xE0, i % 20),
            amount: 100,
        }]);
        benign.push(vec![Effect::Call {
            contract: addr(0xE0, i % 20),
            method: "stake".into(),
        }]);
        benign.push(vec![
            Effect::Call {
                contract: addr(0xE0, i % 20),
                method: "swap".into(),
            },
            Effect::Transfer {
                token: NATIVE,
                to: addr(0xE0, i % 20),
                amount: 500,
            },
        ]);
    }
    let alarms = benign
        .iter()
        .filter(|e| !review(e, e, &ctx).is_empty())
        .count();
    println!(
        "{} ordinary transactions: {alarms} false alarms",
        benign.len()
    );
    assert_eq!(benign.len(), 100);
    assert_eq!(
        alarms, 0,
        "ordinary transfers to contacts, bounded approvals to verified contracts and verified calls must not warn"
    );
}

#[test]
fn rendering_says_the_dangerous_part_in_capitals() {
    let e = Effect::Approve {
        token: addr(0x70, 0),
        spender: addr(0xBA, 1),
        amount: UNLIMITED,
    };
    assert!(render(&e).contains("ALL of your token"), "{}", render(&e));
    let e = Effect::ApproveAll {
        collection: addr(0x71, 0),
        operator: addr(0xBA, 1),
    };
    assert!(render(&e).contains("EVERY item"));
}
