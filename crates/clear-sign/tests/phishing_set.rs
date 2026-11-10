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

use maya_amount::Amount;
use std::collections::BTreeSet;

use maya_clear_sign::{
    Address, Context, Effect, Network, UNLIMITED, Warning, drains, grouped, render, review,
    text_look_alike,
};

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
        balances: vec![
            (NATIVE, Amount::from_u64(1_000_000)),
            (addr(0x70, 0), Amount::from_u64(5_000)),
        ],
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
                amount: Amount::from_u64(10),
            }],
            vec![Effect::Approve {
                token,
                spender: thief,
                amount: UNLIMITED.saturating_add(Amount::from_u64(u64::from(i))),
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
                amount: Amount::from_u128(100 + u128::from(i)),
            }],
            vec![Effect::Permit {
                token,
                spender: thief,
                amount: Amount::from_u128(100 + u128::from(i)),
            }],
            Warning::PermitSignature,
        ),
        (
            "address poisoning",
            vec![Effect::Transfer {
                token: NATIVE,
                to: poison(i % 20),
                amount: Amount::from_u64(1_000),
            }],
            vec![Effect::Transfer {
                token: NATIVE,
                to: poison(i % 20),
                amount: Amount::from_u64(1_000),
            }],
            Warning::LookAlikeRecipient,
        ),
        (
            "balance drain",
            vec![Effect::Transfer {
                token: NATIVE,
                to: thief,
                amount: Amount::from_u128(900_000 + 5_000 * u128::from(i)),
            }],
            vec![Effect::Transfer {
                token: NATIVE,
                to: thief,
                amount: Amount::from_u128(900_000 + 5_000 * u128::from(i)),
            }],
            Warning::DrainsBalance,
        ),
        (
            "claim mismatch",
            vec![Effect::Transfer {
                token: NATIVE,
                to: addr(0xC0, i % 20),
                amount: Amount::from_u64(10),
            }],
            vec![Effect::Transfer {
                token: NATIVE,
                to: thief,
                amount: Amount::from_u64(10),
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
                    amount: Amount::from_u64(50),
                },
            ],
            Warning::ClaimMismatch,
        ),
        (
            "malicious upgrade (Bybit shape)",
            vec![Effect::Transfer {
                token: NATIVE,
                to: addr(0xC0, i % 20),
                amount: Amount::from_u64(0),
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
                amount: Amount::from_u64(4_800),
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
            amount: Amount::from_u128(1_000 + u128::from(i)),
        }]);
        benign.push(vec![Effect::Approve {
            token: addr(0x70, 0),
            spender: addr(0xE0, i % 20),
            amount: Amount::from_u64(100),
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
                amount: Amount::from_u64(500),
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
    let text = render(&e, Network::Mainnet);
    assert!(text.contains("ALL of your token"), "{text}");
    let e = Effect::ApproveAll {
        collection: addr(0x71, 0),
        operator: addr(0xBA, 1),
    };
    assert!(render(&e, Network::Mainnet).contains("EVERY item"));
}

/// The Bech32 generator g(x) = x^6 + 29x^5 + 22x^4 + 20x^3 + 21x^2 + 29x + 18
/// (BIP-173). XOR-ing a shifted copy into the data leaves the checksum
/// unchanged: the strongest poisoning a person reading the ends can face.
const GENERATOR: [u8; 7] = [1, 29, 22, 20, 21, 29, 18];

/// A look-alike of `of` whose Bech32m text matches at both ends, with the
/// change at payload symbols `at..at + 7`.
fn bech32m_poison(of: &Address, at: usize) -> Address {
    let text = maya_address::encode(Network::Mainnet, of);
    let (hrp, mut data) = maya_address::bech32m::split(&text).unwrap();
    // data[0] is the version symbol; the payload starts at 1.
    for (d, g) in data[1 + at..1 + at + GENERATOR.len()].iter_mut().zip(GENERATOR) {
        *d ^= g;
    }
    let forged = maya_address::bech32m::join(&hrp, &data);
    assert_eq!(forged[forged.len() - 6..], text[text.len() - 6..], "checksum kept");
    maya_address::decode(Network::Mainnet, &forged).unwrap()
}

#[test]
fn every_bech32m_look_alike_of_a_contact_is_flagged() {
    let ctx = context();
    let mut missed = Vec::new();
    let mut bytes_rule_would_miss = 0;
    for i in 0..20u8 {
        let contact = addr(0xC0, i);
        let to = bech32m_poison(&contact, 6 + 2 * usize::from(i));
        assert_ne!(to, contact);
        assert!(text_look_alike(&contact, &to));
        if to[30..] != contact[30..] {
            bytes_rule_would_miss += 1;
        }
        let e = vec![Effect::Transfer {
            token: NATIVE,
            to,
            amount: Amount::from_u64(1_000),
        }];
        let w = review(&e, &e, &ctx);
        if !w.contains(&Warning::LookAlikeRecipient) {
            missed.push(format!("contact {i}: got {w:?}"));
        }
    }
    println!(
        "20 Bech32m look-alikes (same first 6 payload + last 6 characters): missed {}; \
         {bytes_rule_would_miss} of them differ in the last two bytes, so the old byte rule alone missed them",
        missed.len()
    );
    assert!(missed.is_empty(), "{missed:#?}");
    assert!(bytes_rule_would_miss > 0, "the family must exercise the text rule");
}

#[test]
fn the_bech32m_look_alike_false_alarm_rate_is_measured() {
    let ctx = context();
    // splitmix64: deterministic stand-ins for unrelated addresses.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let trials = 10_000;
    let mut alarms = 0;
    for _ in 0..trials {
        let mut to = [0u8; 32];
        for chunk in to.chunks_mut(8) {
            chunk.copy_from_slice(&next().to_le_bytes());
        }
        let e = vec![Effect::Transfer {
            token: NATIVE,
            to,
            amount: Amount::from_u64(1_000),
        }];
        if review(&e, &e, &ctx).contains(&Warning::LookAlikeRecipient) {
            alarms += 1;
        }
    }
    println!(
        "{trials} unrelated recipients against {} contacts ({} pair checks): {alarms} look-alike false alarms",
        ctx.contacts.len(),
        trials * ctx.contacts.len()
    );
}

/// Grinds an address whose grouped Bech32m shows the same first and last
/// group as `of` (`maya1 qXXX … XXX`): what a person compares at a glance.
/// Keeping the first two bytes fixes the first three payload symbols, so only
/// the three trailing checksum symbols (15 bits) are ground. Returns the
/// address and the tries it took.
fn grind_displayed_ends(of: &Address, mut state: u64) -> (Address, u64) {
    let shown = grouped(Network::Mainnet, of);
    let groups: Vec<&str> = shown.split(' ').collect();
    let (first, last) = (groups[1].to_string(), groups[groups.len() - 1].to_string());
    let mut tries = 0;
    loop {
        tries += 1;
        let mut a = *of;
        for chunk in a[2..].chunks_mut(8) {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
        let candidate = grouped(Network::Mainnet, &a);
        let g: Vec<&str> = candidate.split(' ').collect();
        if a != *of && g[1] == first && g[g.len() - 1] == last {
            return (a, tries);
        }
    }
}

#[test]
fn a_look_alike_ground_to_the_displayed_first_and_last_group_is_flagged() {
    let ctx = context();
    let mut missed = Vec::new();
    let mut total_tries = 0;
    for i in 0..5u8 {
        let contact = addr(0xC0, i);
        let (to, tries) = grind_displayed_ends(&contact, 0xA11C_E000 + u64::from(i));
        total_tries += tries;
        let e = vec![Effect::Transfer {
            token: NATIVE,
            to,
            amount: Amount::from_u64(1_000),
        }];
        if !review(&e, &e, &ctx).contains(&Warning::LookAlikeRecipient) {
            missed.push(format!(
                "contact {i}: {} vs {}",
                grouped(Network::Mainnet, &contact),
                grouped(Network::Mainnet, &to)
            ));
        }
    }
    println!(
        "5 addresses ground to a contact's displayed first and last group in {total_tries} tries; missed {}",
        missed.len()
    );
    assert!(missed.is_empty(), "{missed:#?}");
}

#[test]
fn the_drain_check_holds_at_the_90_percent_boundary_within_256_bits() {
    // Small values: 9 of 10 drains, 8 of 10 does not.
    assert!(drains(Amount::from_u64(9), Amount::from_u64(10)));
    assert!(!drains(Amount::from_u64(8), Amount::from_u64(10)));
    assert!(!drains(Amount::from_u64(5), Amount::ZERO));
    // ceil(0.9 × 11) = 10.
    assert!(drains(Amount::from_u64(10), Amount::from_u64(11)));
    assert!(!drains(Amount::from_u64(9), Amount::from_u64(11)));
    // Near the top, where 10a and 9b overflow 256 bits: a >= b - b/10.
    let b = Amount::MAX;
    let edge = b.saturating_sub(b.checked_div(Amount::from_u64(10)).unwrap());
    assert!(drains(edge, b));
    assert!(!drains(edge.saturating_sub(Amount::ONE), b));
    assert!(drains(Amount::MAX, Amount::MAX));
    // Against the exact 512-bit products (the old rule, kept here as the oracle).
    let oracle = |a: Amount, b: Amount| {
        !b.is_zero() && a.mul_wide(Amount::from_u64(100)) >= b.mul_wide(Amount::from_u64(90))
    };
    let mut checked = 0;
    for shift in [0u32, 1, 63, 64, 128, 200, 252, 253, 255] {
        let base = Amount::MAX.shr(shift);
        for db in 0..12u64 {
            let b = base.saturating_sub(Amount::from_u64(db));
            let e = b.saturating_sub(b.checked_div(Amount::from_u64(10)).unwrap());
            for a in [
                e.saturating_sub(Amount::from_u64(2)),
                e.saturating_sub(Amount::ONE),
                e,
                e.saturating_add(Amount::ONE),
            ] {
                assert_eq!(drains(a, b), oracle(a, b), "a={a} b={b}");
                checked += 1;
            }
        }
    }
    println!("drain boundary: {checked} pairs agree with the 512-bit oracle");
}

#[test]
fn rendering_shows_the_full_grouped_bech32m_address() {
    let to = addr(0xBA, 1);
    let text = render(
        &Effect::Transfer {
            token: NATIVE,
            to,
            amount: Amount::from_u64(5),
        },
        Network::Testnet,
    );
    let shown = grouped(Network::Testnet, &to);
    assert!(text.contains(&shown), "{text}");
    assert!(shown.starts_with("tmaya1 "), "{shown}");
    assert!(shown.split(' ').skip(1).all(|g| g.len() <= 4), "{shown}");
    assert_eq!(
        shown.replace(' ', ""),
        maya_address::encode(Network::Testnet, &to)
    );
    assert!(!text.contains('…'), "never abbreviated: {text}");
}
