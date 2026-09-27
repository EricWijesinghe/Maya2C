//! A real Ethereum mainnet finality proof, verified offline (Master Prompt 25).
//!
//! `tests/fixtures/eth_beacon_finality.json` was fetched by
//! `scripts/eth_beacon_fixture.py`: a light-client bootstrap, a finality
//! update, and the execution header the finalized beacon block commits to.
//! Verified here, with no network:
//!
//! bootstrap root → committee branch → 2/3 BLS signature over the attested
//! header → finality branch → execution-payload branch → `block_hash` →
//! Keccak of the full execution header.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_interop::beacon::{
    self, BeaconHeader, ExecutionPayloadHeader, FinalityProof, LightClientError, Root,
    SyncCommittee,
};
use maya_interop::eth::{self, Header};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/eth_beacon_finality.json");

fn bytes(v: &Value) -> Vec<u8> {
    let s = v.as_str().unwrap().trim_start_matches("0x");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn arr<const N: usize>(v: &Value) -> [u8; N] {
    bytes(v).try_into().unwrap()
}

fn dec(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

fn hex_u(v: &Value) -> u128 {
    u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn beacon_header(v: &Value) -> BeaconHeader {
    BeaconHeader {
        slot: dec(&v["slot"]),
        proposer_index: dec(&v["proposer_index"]),
        parent_root: arr(&v["parent_root"]),
        state_root: arr(&v["state_root"]),
        body_root: arr(&v["body_root"]),
    }
}

fn u256_le(decimal: &str) -> Root {
    // Base fees fit u128; the SSZ field is a little-endian uint256.
    let v: u128 = decimal.parse().unwrap();
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&v.to_le_bytes());
    out
}

fn execution(v: &Value) -> ExecutionPayloadHeader {
    ExecutionPayloadHeader {
        parent_hash: arr(&v["parent_hash"]),
        fee_recipient: arr(&v["fee_recipient"]),
        state_root: arr(&v["state_root"]),
        receipts_root: arr(&v["receipts_root"]),
        logs_bloom: bytes(&v["logs_bloom"]),
        prev_randao: arr(&v["prev_randao"]),
        block_number: dec(&v["block_number"]),
        gas_limit: dec(&v["gas_limit"]),
        gas_used: dec(&v["gas_used"]),
        timestamp: dec(&v["timestamp"]),
        extra_data: bytes(&v["extra_data"]),
        base_fee_per_gas: u256_le(v["base_fee_per_gas"].as_str().unwrap()),
        block_hash: arr(&v["block_hash"]),
        transactions_root: arr(&v["transactions_root"]),
        withdrawals_root: arr(&v["withdrawals_root"]),
        blob_gas_used: dec(&v["blob_gas_used"]),
        excess_blob_gas: dec(&v["excess_blob_gas"]),
    }
}

fn branch(v: &Value) -> Vec<Root> {
    v.as_array().unwrap().iter().map(arr).collect()
}

struct Loaded {
    trusted: Root,
    bootstrap: BeaconHeader,
    committee: SyncCommittee,
    committee_branch: Vec<Root>,
    proof: FinalityProof,
    fork: [u8; 4],
    gvr: Root,
    exec_json: Value,
}

fn load() -> Loaded {
    let f: Value = serde_json::from_str(FIXTURE).unwrap();
    let b = &f["bootstrap"]["data"];
    let u = &f["finality_update"]["data"];
    let committee = SyncCommittee {
        pubkeys: b["current_sync_committee"]["pubkeys"]
            .as_array()
            .unwrap()
            .iter()
            .map(arr)
            .collect(),
        aggregate_pubkey: arr(&b["current_sync_committee"]["aggregate_pubkey"]),
    };
    Loaded {
        trusted: arr(&f["finalized_root_from_api"]),
        bootstrap: beacon_header(&b["header"]["beacon"]),
        committee,
        committee_branch: branch(&b["current_sync_committee_branch"]),
        proof: FinalityProof {
            attested: beacon_header(&u["attested_header"]["beacon"]),
            finalized: beacon_header(&u["finalized_header"]["beacon"]),
            finality_branch: branch(&u["finality_branch"]),
            execution: execution(&u["finalized_header"]["execution"]),
            execution_branch: branch(&u["finalized_header"]["execution_branch"]),
            participation: arr(&u["sync_aggregate"]["sync_committee_bits"]),
            signature: arr(&u["sync_aggregate"]["sync_committee_signature"]),
            signature_slot: dec(&u["signature_slot"]),
        },
        fork: arr(&f["fork"]["current_version"]),
        gvr: arr(&f["genesis_validators_root"]),
        exec_json: f["execution_header"].clone(),
    }
}

fn execution_header(j: &Value) -> Header {
    let opt = |k: &str| j.get(k).filter(|v| !v.is_null());
    Header {
        parent_hash: arr(&j["parentHash"]),
        ommers_hash: arr(&j["sha3Uncles"]),
        beneficiary: arr(&j["miner"]),
        state_root: arr(&j["stateRoot"]),
        transactions_root: arr(&j["transactionsRoot"]),
        receipts_root: arr(&j["receiptsRoot"]),
        logs_bloom: bytes(&j["logsBloom"]),
        difficulty: hex_u(&j["difficulty"]),
        number: u64::try_from(hex_u(&j["number"])).unwrap(),
        gas_limit: u64::try_from(hex_u(&j["gasLimit"])).unwrap(),
        gas_used: u64::try_from(hex_u(&j["gasUsed"])).unwrap(),
        timestamp: u64::try_from(hex_u(&j["timestamp"])).unwrap(),
        extra_data: bytes(&j["extraData"]),
        mix_hash: arr(&j["mixHash"]),
        nonce: arr(&j["nonce"]),
        base_fee: opt("baseFeePerGas").map(hex_u),
        withdrawals_root: opt("withdrawalsRoot").map(arr),
        blob_gas_used: opt("blobGasUsed").map(|v| u64::try_from(hex_u(v)).unwrap()),
        excess_blob_gas: opt("excessBlobGas").map(|v| u64::try_from(hex_u(v)).unwrap()),
        parent_beacon_root: opt("parentBeaconBlockRoot").map(arr),
        requests_hash: opt("requestsHash").map(arr),
    }
}

#[test]
fn a_real_mainnet_finality_update_verifies_down_to_the_execution_header() {
    let l = load();
    beacon::verify_bootstrap(l.trusted, &l.bootstrap, &l.committee, &l.committee_branch)
        .expect("committee in the trusted bootstrap state");
    let block_hash =
        beacon::verify_finality(&l.committee, l.bootstrap.slot, &l.proof, l.fork, l.gvr)
            .expect("finality proof");
    let exec = execution_header(&l.exec_json);
    assert_eq!(
        eth::hash(&exec),
        block_hash,
        "the execution header is the finalized one"
    );
    assert_eq!(exec.number, l.proof.execution.block_number);
    let signers = l
        .proof
        .participation
        .iter()
        .map(|b| b.count_ones())
        .sum::<u32>();
    println!(
        "mainnet: {signers}/512 sync-committee signers finalized beacon slot {} → execution block {} \
         ({}), whose Keccak header hash verifies",
        l.proof.finalized.slot,
        exec.number,
        l.exec_json["hash"].as_str().unwrap()
    );
}

#[test]
fn a_forged_proof_fails_at_the_step_it_forges() {
    let l = load();
    let verify = |p: &FinalityProof| {
        beacon::verify_finality(&l.committee, l.bootstrap.slot, p, l.fork, l.gvr)
    };

    let mut p = l.proof.clone();
    p.attested.slot += 1;
    assert_eq!(
        verify(&p),
        Err(LightClientError::Signature),
        "signed header changed"
    );

    let mut p = l.proof.clone();
    p.finalized.state_root[0] ^= 1;
    assert_eq!(verify(&p), Err(LightClientError::FinalityBranch));

    let mut p = l.proof.clone();
    p.execution.block_number += 1;
    assert_eq!(verify(&p), Err(LightClientError::ExecutionBranch));

    let mut p = l.proof.clone();
    p.participation = [0; 64];
    assert_eq!(verify(&p), Err(LightClientError::Participation));

    let mut wrong_fork = l.fork;
    wrong_fork[0] ^= 1;
    assert_eq!(
        beacon::verify_finality(&l.committee, l.bootstrap.slot, &l.proof, wrong_fork, l.gvr),
        Err(LightClientError::Signature),
        "a signature is bound to its fork"
    );

    let mut other = l.bootstrap.clone();
    other.slot += 1;
    assert_eq!(
        beacon::verify_bootstrap(l.trusted, &other, &l.committee, &l.committee_branch),
        Err(LightClientError::UntrustedBootstrap)
    );
}
