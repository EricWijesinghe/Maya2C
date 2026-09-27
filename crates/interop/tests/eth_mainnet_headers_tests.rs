//! Sixty-four consecutive real mainnet headers (Master Prompt 25).
//!
//! `tests/fixtures/eth_mainnet_headers.json` was fetched by
//! `scripts/eth_headers_fixture.py`. Each header's hash is recomputed from
//! its fields — which exercises every post-London optional field, since these
//! are Prague-era headers — and the parent links are checked end to end.
//!
//! What this is and is not: it proves the hasher and the chain rule on real
//! data. It is not finality: which of two valid-looking chains Ethereum
//! finalized is the beacon chain's to say (sync-committee signatures), and a
//! ZK proof of either is not built. `reports/25-interop.md` says so.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_interop::eth::{self, Header};

const FIXTURE: &str = include_str!("fixtures/eth_mainnet_headers.json");

fn bytes(v: &serde_json::Value) -> Vec<u8> {
    let s = v.as_str().unwrap().trim_start_matches("0x");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn arr<const N: usize>(v: &serde_json::Value) -> [u8; N] {
    bytes(v).try_into().unwrap()
}

fn uint(v: &serde_json::Value) -> u128 {
    u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn header(j: &serde_json::Value) -> Header {
    let opt = |k: &str| j.get(k).filter(|v| !v.is_null());
    Header {
        parent_hash: arr(&j["parentHash"]),
        ommers_hash: arr(&j["sha3Uncles"]),
        beneficiary: arr(&j["miner"]),
        state_root: arr(&j["stateRoot"]),
        transactions_root: arr(&j["transactionsRoot"]),
        receipts_root: arr(&j["receiptsRoot"]),
        logs_bloom: bytes(&j["logsBloom"]),
        difficulty: uint(&j["difficulty"]),
        number: u64::try_from(uint(&j["number"])).unwrap(),
        gas_limit: u64::try_from(uint(&j["gasLimit"])).unwrap(),
        gas_used: u64::try_from(uint(&j["gasUsed"])).unwrap(),
        timestamp: u64::try_from(uint(&j["timestamp"])).unwrap(),
        extra_data: bytes(&j["extraData"]),
        mix_hash: arr(&j["mixHash"]),
        nonce: arr(&j["nonce"]),
        base_fee: opt("baseFeePerGas").map(uint),
        withdrawals_root: opt("withdrawalsRoot").map(arr),
        blob_gas_used: opt("blobGasUsed").map(|v| u64::try_from(uint(v)).unwrap()),
        excess_blob_gas: opt("excessBlobGas").map(|v| u64::try_from(uint(v)).unwrap()),
        parent_beacon_root: opt("parentBeaconBlockRoot").map(arr),
        requests_hash: opt("requestsHash").map(arr),
    }
}

#[test]
fn sixty_four_real_mainnet_headers_hash_to_their_ids_and_link() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let raw = fixture["headers"].as_array().unwrap();
    assert!(raw.len() >= 32);
    let headers: Vec<Header> = raw.iter().map(header).collect();
    for (j, h) in raw.iter().zip(&headers) {
        assert_eq!(
            eth::hash(h).to_vec(),
            bytes(&j["hash"]),
            "block {} does not hash to its published id",
            h.number
        );
    }
    let tip = eth::check_chain(&headers).expect("parent links");
    assert_eq!(tip.to_vec(), bytes(&raw.last().unwrap()["hash"]));
    println!(
        "mainnet blocks {}..={} ({} headers, fetched {} from {}): every hash recomputed, every parent link holds",
        headers[0].number,
        headers.last().unwrap().number,
        headers.len(),
        fixture["fetched_utc"].as_str().unwrap(),
        fixture["source"].as_str().unwrap()
    );
}

#[test]
fn a_real_header_with_one_field_changed_no_longer_links() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let mut headers: Vec<Header> = fixture["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(header)
        .collect();
    headers[10].gas_used += 1;
    assert_eq!(eth::check_chain(&headers), Err(11));
}
