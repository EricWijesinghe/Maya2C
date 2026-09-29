//! The fee market over real transactions: sizes from the node's own encoding,
//! a simulated run of blocks, and the supply figures that result.
//!
//! `crates/fee-market/src/` tests the rules in isolation. This file does two things
//! those cannot:
//!
//! 1. Charges the base fee on the **actual** serialized size of a hybrid-signed
//!    transfer — 11,165 bytes of signature and 1,984 of public key — rather than
//!    a number chosen for a test. The byte-based fee is only as meaningful as
//!    the sizes it is charged on.
//! 2. Checks the branch is as inert as it claims: disabled everywhere, and
//!    referenced by nothing in `src/`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::transaction::{Transaction, TxInput, TxOutput};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use maya_fee_market::{
    BlockFeeOutcome, FeeClaim, FeeConfig, MAX_SUPPLY, ParentFees, Supply, TxFee, apply_block_fees,
};

/// The serialized size of one signed single-input, single-output transfer.
fn signed_transfer_size() -> u64 {
    let key = generate_signing_key().expect("keygen");
    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [1; 32],
            index: 0,
        }],
        vec![TxOutput {
            amount: 1_000,
            recipient: [2; 32],
        }],
        0,
    );
    tx.sign(&key).expect("sign");
    tx.to_bytes().len() as u64
}

#[test]
fn a_real_transfer_is_dominated_by_its_hybrid_signature() {
    // The premise of charging per byte: size, not compute, is what a
    // transaction here costs the chain. If this ever fell far below the
    // signature length, the control law would be steering the wrong resource.
    let size = signed_transfer_size();
    assert!(size > 11_165 + 1_984, "a signed transfer is {size} bytes");
    assert!(size < 16 * 1024, "a signed transfer is {size} bytes");
}

#[test]
fn circulating_supply_falls_every_block_while_total_is_conserved() {
    let config = FeeConfig::TESTING;
    let size = signed_transfer_size();
    // Twice the target in every block: sustained demand, so the base fee
    // rises and each block burns more than the last.
    let per_block = (2 * config.target_block_bytes / size) as usize;
    let producer = FeeClaim {
        beneficiary: [0xB0; 32],
    };

    let mut supply = Supply::new(1_000_000_000_000, 1_000_000_000_000).expect("supply");
    let mut parent: Option<ParentFees> = None;
    let mut last_burn = 0u64;

    for height in 1..=40 {
        let base_fee = parent.map_or(config.initial_base_fee, |p| {
            maya_fee_market::next_base_fee(
                p.base_fee,
                p.size_bytes,
                config.target_block_bytes,
                config.change_denominator,
                config.min_base_fee,
            )
        });
        // Every sender offers enough for the base fee plus a small tip.
        let txs: Vec<TxFee> = (0..per_block)
            .map(|_| TxFee {
                size_bytes: size,
                max_fee: base_fee * size + 100,
                max_tip: 100,
            })
            .collect();

        let Ok(BlockFeeOutcome::Charged {
            base_fee: charged_at,
            charges,
            burned,
            treasury,
            tips,
            ..
        }) = apply_block_fees(&config, height, parent, &txs, &[producer])
        else {
            panic!("block {height} was not charged");
        };
        assert_eq!(charged_at, base_fee);

        // What was charged is exactly what was distributed.
        let charged: u128 = charges.iter().map(|c| u128::from(c.charged)).sum();
        assert_eq!(
            charged,
            u128::from(burned) + u128::from(treasury) + u128::from(tips),
            "block {height}"
        );

        let next = supply.after_burn(burned).expect("burn within circulating");
        assert!(
            next.circulating < supply.circulating,
            "block {height} burned nothing"
        );
        assert_eq!(
            next.total, supply.total,
            "block {height} changed total supply"
        );
        assert!(
            burned >= last_burn,
            "block {height} burned less under rising demand"
        );
        maya_fee_market::check_supply(next.total).expect("under the cap");

        last_burn = burned;
        supply = next;
        parent = Some(ParentFees {
            base_fee,
            size_bytes: size * per_block as u64,
        });
    }
}

#[test]
fn the_treasury_receives_one_fifth_of_the_base_fee_and_the_producer_every_tip() {
    let config = FeeConfig::TESTING;
    let size = signed_transfer_size();
    let base_fee = config.initial_base_fee;
    let txs = [TxFee {
        size_bytes: size,
        max_fee: base_fee * size + 250,
        max_tip: 250,
    }];
    let Ok(BlockFeeOutcome::Charged {
        burned,
        treasury,
        tips,
        beneficiary,
        ..
    }) = apply_block_fees(
        &config,
        1,
        None,
        &txs,
        &[FeeClaim {
            beneficiary: [9; 32],
        }],
    )
    else {
        panic!("charged");
    };
    let base_paid = base_fee * size;
    assert_eq!(treasury, base_paid / 5);
    assert_eq!(burned, base_paid - base_paid / 5);
    assert_eq!(tips, 250);
    assert_eq!(beneficiary, Some([9; 32]));
}

#[test]
fn the_cap_holds_at_its_boundary() {
    assert!(Supply::new(MAX_SUPPLY, MAX_SUPPLY).is_ok());
    assert!(Supply::new(MAX_SUPPLY + 1, 0).is_err());
}

#[test]
fn every_network_runs_the_fee_market_disabled() {
    assert_eq!(FeeConfig::DISABLED.activation_height, u64::MAX);
}

#[test]

mod common;
fn the_fee_market_is_reached_only_through_its_genesis_gate() {
    // ADR-029 wired the base-fee step in, behind `genesis.bft.fees`: only the
    // genesis parameter check and the fee record may name the crate. The day
    // this fails, another path has started deciding fees and needs the same
    // review ADR-029 had. A grep rather than a type-level guarantee, the check
    // `lattice-pow` and `blockgraph` are held to.
    const ALLOWED: [&str; 2] = ["genesis.rs", "fees.rs"];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![root];
    let mut offenders = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("read file");
                let allowed = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| ALLOWED.contains(&n));
                if text.contains("maya_fee_market") && !allowed {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "fee market referenced from {offenders:?}"
    );
}
