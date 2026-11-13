//! Golden HTML for the screenshot-diff job.
//!
//! Renders each explorer page from fixed data and compares it with the copy
//! committed under `design/visual/explorer/`. CI screenshots those files in the
//! pinned Playwright image and diffs them against baselines, so a change to a
//! page shows up as a pixel diff there. Regenerate after an intended change with
//! `UPDATE_FIXTURES=1 cargo test -p maya-explorer --test fixture_tests`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use custom_l1_node::rpc::validators::{ValidatorInfo, ValidatorsInfo};
use maya_address::Network;
use maya_explorer::model::{HashratePoint, IndexedBlock, IndexedTx, NetworkStats};
use maya_explorer::ui;
use primitive_types::U256;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../design/visual/explorer")
}

fn blocks(count: i64) -> Vec<IndexedBlock> {
    // Newest first, 15 s apart with an uneven gap so the chart has a shape.
    (0..count)
        .rev()
        .map(|height| IndexedBlock {
            height,
            id: format!("{height:064x}"),
            prev_hash: format!("{:064x}", height.saturating_sub(1)),
            state_root: "ab".repeat(32),
            timestamp: 1_700_000_000 + height * 15 + (height % 3) * 4,
            nonce: height * 7,
            difficulty_target: format!("00{}", "ff".repeat(31)),
            tx_count: i32::try_from((height * 5) % 9).expect("small"),
            work: "256".to_string(),
        })
        .collect()
}

fn stats() -> NetworkStats {
    NetworkStats {
        height: 12,
        blocks_indexed: 13,
        transactions_indexed: 40,
        hashrate: 1_500_000.0,
        average_block_time: 15.0,
        difficulty_target: format!("00{}", "ff".repeat(31)),
    }
}

fn transaction(height: i64, index: usize) -> IndexedTx {
    IndexedTx {
        txid: format!("{height:032x}{index:032x}"),
        height,
        sender: "aa".repeat(32),
        nonce: index as i64,
        output_count: 1,
        total_out: (500 + index as i64).to_string(),
        signed: true,
        kind: "transfer".to_string(),
        summary: "Send 500 units.".to_string(),
        careful: false,
        explanation: String::new(),
        outputs: Vec::new(),
    }
}

fn hashrate_series() -> Vec<HashratePoint> {
    (1..=12)
        .map(|height: i64| HashratePoint {
            height,
            timestamp: 1_700_000_000 + height * 15,
            hashrate: 1_000_000.0 + (height % 4) as f64 * 150_000.0,
        })
        .collect()
}

fn validators() -> ValidatorsInfo {
    let validator = |n: u8, in_committee: bool, self_bond: &str, delegated: &str| ValidatorInfo {
        id: format!("{n:064x}"),
        operator: format!("{:064x}", u32::from(n) + 100),
        self_bond: self_bond.to_string(),
        delegated: delegated.to_string(),
        commission_bps: 500,
        status: "bonded".to_string(),
        jailed_until: None,
        in_committee,
    };
    ValidatorsInfo {
        epoch: 4,
        epoch_blocks: 100,
        committee: 3,
        total: 4,
        validators: vec![
            validator(1, true, "5000000", "1000000"),
            validator(2, true, "4000000", "0"),
            validator(3, true, "3000000", "2000000"),
            validator(4, false, "1000000", "0"),
        ],
    }
}

/// Every page the screenshot job covers, by file stem.
fn pages() -> Vec<(&'static str, String)> {
    let address = [0xaa_u8; 32];
    let text = maya_address::encode(Network::Testnet, &address);
    vec![
        ("dashboard", ui::dashboard_page(stats(), blocks(10))),
        ("blocks", ui::blocks_page(blocks(10))),
        (
            "account",
            ui::account_page(
                "aa".repeat(32),
                Some(text),
                Some(U256::from(5_000_u64)),
                Some(3),
                vec![transaction(9, 0), transaction(8, 1)],
                None,
            ),
        ),
        ("mining", ui::mining_page(stats(), hashrate_series())),
        ("validators", ui::validators_page(Some(validators()), None)),
    ]
}

#[test]
fn committed_fixtures_match_the_renderer() {
    let update = std::env::var_os("UPDATE_FIXTURES").is_some();
    let dir = fixture_dir();
    if update {
        fs::create_dir_all(&dir).expect("create fixture dir");
    }

    for (name, html) in pages() {
        let path = dir.join(format!("{name}.html"));
        if update {
            fs::write(&path, &html).expect("write fixture");
            continue;
        }
        let committed = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("{}: {e}; regenerate with UPDATE_FIXTURES=1", path.display())
        });
        assert!(
            committed == html,
            "{name} differs from {}; if the change is intended, regenerate with UPDATE_FIXTURES=1",
            path.display()
        );
    }
}
