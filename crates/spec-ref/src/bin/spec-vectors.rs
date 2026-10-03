//! Writes `spec/tests/*.json` from the reference implementation.
//!
//! `cargo run -p maya-spec-ref --bin spec-vectors` regenerates them; CI runs it
//! with `--check`, which fails if the committed files differ from what the
//! reference produces now. `spec/tests/keys.json` is the one input: seeds and
//! the hybrid public keys the node derives from them. This binary recomputes
//! every address from its key (TX-2) and refuses to run if one disagrees.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use maya_spec_ref::stf::{Account, Transfer, apply_block};
use maya_spec_ref::wire::{self, Frame};
use maya_spec_ref::{Address, consensus, fees, header, root};
use serde_json::{Value, json};

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 0x0f)]),
            ]
        })
        .collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Every u64 in a vector is a decimal string: JSON numbers above 2^53 are
/// rounded by most parsers (JavaScript's among them), and a vector that means
/// something different in each language is not language-neutral.
fn num(v: &Value) -> u64 {
    v.as_str()
        .expect("u64 fields are decimal strings")
        .parse()
        .expect("decimal u64")
}

fn spec_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests")
}

/// Key name → address, from keys.json, each address recomputed from its key.
fn keys() -> BTreeMap<String, Address> {
    let text = std::fs::read_to_string(spec_dir().join("keys.json")).expect("spec/tests/keys.json");
    let doc: Value = serde_json::from_str(&text).unwrap();
    let mut out = BTreeMap::new();
    for k in doc["keys"].as_array().unwrap() {
        let pk = unhex(k["public_key"].as_str().unwrap());
        let claimed: Address = unhex(k["address"].as_str().unwrap()).try_into().unwrap();
        assert_eq!(
            wire::address_of(&pk),
            claimed,
            "TX-2: keys.json address for {} is not blake3(domain ‖ pk)",
            k["name"]
        );
        out.insert(k["name"].as_str().unwrap().to_string(), claimed);
    }
    out
}

/// A named key (`k0`) or a literal 64-hex-digit address.
fn resolve(keys: &BTreeMap<String, Address>, name: &str) -> Address {
    keys.get(name)
        .copied()
        .unwrap_or_else(|| unhex(name).try_into().expect("address or key name"))
}

/// One state-transition case: pre-state and block as JSON, expectation computed.
fn stf_case(
    keys: &BTreeMap<String, Address>,
    id: &str,
    rules: &[&str],
    pre: &Value,
    block: &Value,
) -> Value {
    let mut state = BTreeMap::new();
    for a in pre.as_array().unwrap() {
        let acc = Account {
            balance: num(&a["balance"]),
            nonce: num(&a["nonce"]),
        };
        state.insert(resolve(keys, a["account"].as_str().unwrap()), acc);
    }
    let txs: Vec<Transfer> = block
        .as_array()
        .unwrap()
        .iter()
        .map(|t| Transfer {
            sender: resolve(keys, t["from"].as_str().unwrap()),
            signature_valid: t["signature"] == "valid",
            nonce: num(&t["nonce"]),
            outputs: t["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| (resolve(keys, o["to"].as_str().unwrap()), num(&o["amount"])))
                .collect(),
        })
        .collect();
    let expect = match apply_block(&state, &txs) {
        Ok(post) => {
            let accounts: Vec<Value> = post.iter().map(|(a, acc)| json!({"address": hex(a), "balance": acc.balance.to_string(), "nonce": acc.nonce.to_string()})).collect();
            json!({"result": "ok", "post": accounts, "post_root": hex(&root::accounts_root(&post))})
        }
        Err((index, why)) => json!({"result": "error", "error": why.name(), "tx_index": index}),
    };
    json!({"id": id, "rules": rules, "pre": pre.clone(), "pre_root": hex(&root::accounts_root(&state)), "block": block.clone(), "expect": expect})
}

fn tx(from: &str, nonce: u64, outputs: &[(&str, u64)]) -> Value {
    let outs: Vec<Value> = outputs
        .iter()
        .map(|(to, amount)| json!({"to": to, "amount": amount.to_string()}))
        .collect();
    json!({"from": from, "nonce": nonce.to_string(), "outputs": outs, "signature": "valid"})
}

fn acct(name: &str, balance: u64, nonce: u64) -> Value {
    json!({"account": name, "balance": balance.to_string(), "nonce": nonce.to_string()})
}

const FRESH: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn state_transitions(k: &BTreeMap<String, Address>) -> Value {
    let mut bad_sig = tx("k0", 0, &[("k1", 5)]);
    bad_sig["signature"] = json!("invalid");
    // TX-5: a well-formed signature, made for another chain's genesis — a
    // testnet transfer replayed on mainnet. The executing node must refuse it.
    let mut other_chain = tx("k0", 0, &[("k1", 5)]);
    other_chain["signature"] = json!("other-chain");
    let cases = vec![
        stf_case(
            k,
            "transfer-basic",
            &[
                "TX-1", "STF-1", "STF-3", "STF-4", "STF-5", "ROOT-1", "ROOT-2", "ROOT-4", "ROOT-5",
            ],
            &json!([acct("k0", 1000, 0), acct("k1", 0, 0)]),
            &json!([tx("k0", 0, &[("k1", 10)])]),
        ),
        stf_case(
            k,
            "bad-signature",
            &["TX-1"],
            &json!([acct("k0", 1000, 0)]),
            &json!([bad_sig]),
        ),
        stf_case(
            k,
            "signed-for-another-chain",
            &["TX-1", "TX-5"],
            &json!([acct("k0", 1000, 0)]),
            &json!([other_chain]),
        ),
        stf_case(
            k,
            "replayed-nonce",
            &["STF-1"],
            &json!([acct("k0", 1000, 3)]),
            &json!([tx("k0", 2, &[("k1", 1)])]),
        ),
        stf_case(
            k,
            "skipped-nonce",
            &["STF-1"],
            &json!([acct("k0", 1000, 3)]),
            &json!([tx("k0", 4, &[("k1", 1)])]),
        ),
        stf_case(
            k,
            "outputs-overflow",
            &["STF-2"],
            &json!([acct("k0", u64::MAX, 0)]),
            &json!([tx("k0", 0, &[("k1", u64::MAX), ("k2", 1)])]),
        ),
        stf_case(
            k,
            "multi-output",
            &["STF-2", "ROOT-3"],
            &json!([acct("k0", 100, 0)]),
            &json!([tx("k0", 0, &[("k1", 30), ("k2", 20)])]),
        ),
        stf_case(
            k,
            "overdraft",
            &["STF-3"],
            &json!([acct("k0", 100, 0)]),
            &json!([tx("k0", 0, &[("k1", 101)])]),
        ),
        stf_case(
            k,
            "spend-exact-balance",
            &["STF-3"],
            &json!([acct("k0", 100, 0)]),
            &json!([tx("k0", 0, &[("k1", 100)])]),
        ),
        stf_case(
            k,
            "nonce-overflow",
            &["STF-4"],
            &json!([acct("k0", 100, u64::MAX)]),
            &json!([tx("k0", u64::MAX, &[("k1", 1)])]),
        ),
        stf_case(
            k,
            "recipient-overflow",
            &["STF-5"],
            &json!([acct("k0", 100, 0), acct("k1", u64::MAX - 5, 0)]),
            &json!([tx("k0", 0, &[("k1", 10)])]),
        ),
        stf_case(
            k,
            "self-transfer-nets-zero",
            &["STF-6"],
            &json!([acct("k0", 100, 0)]),
            &json!([tx("k0", 0, &[("k0", 100)])]),
        ),
        stf_case(
            k,
            "credit-creates-account",
            &["STF-7"],
            &json!([acct("k0", 100, 0)]),
            &json!([tx("k0", 0, &[(FRESH, 7)])]),
        ),
        stf_case(
            k,
            "absent-sender-has-nothing",
            &["STF-7"],
            &json!([]),
            &json!([tx("k3", 0, &[("k1", 1)])]),
        ),
        stf_case(
            k,
            "absent-sender-zero-outputs",
            &["STF-7", "ROOT-3"],
            &json!([]),
            &json!([tx("k3", 0, &[])]),
        ),
        stf_case(
            k,
            "chained-in-block",
            &["STF-8"],
            &json!([acct("k0", 100, 0)]),
            &json!([tx("k0", 0, &[("k1", 60)]), tx("k1", 0, &[("k2", 50)])]),
        ),
        stf_case(
            k,
            "one-bad-tx-voids-block",
            &["STF-8"],
            &json!([acct("k0", 100, 0)]),
            &json!([tx("k0", 0, &[("k1", 10)]), tx("k0", 0, &[("k1", 10)])]),
        ),
        stf_case(k, "empty-state", &["ROOT-3"], &json!([]), &json!([])),
    ];
    let cases: Vec<Value> = cases.into_iter().chain(declared_root_cases(k)).collect();
    json!({"format": 1, "u64": "decimal strings", "spec": "spec/03-state.md", "generator": "maya-spec-ref spec-vectors", "cases": cases})
}

fn fee_vectors() -> Value {
    let mut cases = Vec::new();
    for (base, bps) in [
        (1_000u64, 2_000u64),
        (7, 2_000),
        (u64::MAX, 2_000),
        (1_000, 0),
        (1_000, 10_000),
        (1_000, 20_000),
    ] {
        let (burned, treasury) = fees::split(base, bps);
        cases.push(json!({"id": format!("split-{base}-{bps}"), "rules": ["FEE-1"], "fn": "split", "base_fee_paid": base.to_string(), "treasury_bps": bps.to_string(), "expect": {"burned": burned.to_string(), "treasury": treasury.to_string()}}));
    }
    let steps = [
        (
            "at-target",
            1_000u64,
            1_000_000u64,
            1_000_000u64,
            8u64,
            1u64,
            &["FEE-3"][..],
        ),
        ("full-block", 8_000, 2_000_000, 1_000_000, 8, 1, &["FEE-3"]),
        ("empty-block", 8_000, 0, 1_000_000, 8, 1, &["FEE-3"]),
        (
            "rise-at-least-one",
            1,
            1_000_001,
            1_000_000,
            8,
            1,
            &["FEE-3"],
        ),
        (
            "saturate-high",
            u64::MAX,
            2_000_000,
            1_000_000,
            8,
            1,
            &["FEE-3"],
        ),
        ("floor-holds", 1, 0, 1_000_000, 8, 5, &["FEE-4"]),
        ("zero-target", 100, 5_000, 0, 8, 1, &["FEE-2"]),
        ("zero-denominator", 100, 5_000, 1_000, 0, 1, &["FEE-2"]),
    ];
    for (id, parent, size, target, denom, floor, rules) in steps {
        let next = fees::next_base_fee(parent, size, target, denom, floor);
        cases.push(json!({"id": id, "rules": rules, "fn": "next_base_fee", "parent_base_fee": parent.to_string(), "parent_size": size.to_string(), "target": target.to_string(), "denominator": denom.to_string(), "floor": floor.to_string(), "expect": next.to_string()}));
    }
    // FEE-5 at the testing configuration's initial base fee of 10 per byte.
    for (id, size, max_fee) in [
        ("admit-exact", 1_000u64, 10_000u64),
        ("admit-one-short", 1_000, 9_999),
        ("admit-huge-size", u64::MAX, u64::MAX),
    ] {
        let expect = if fees::admits(10, size, max_fee) {
            json!({"result": "ok"})
        } else {
            json!({"result": "error", "error": "Underpriced"})
        };
        cases.push(json!({"id": id, "rules": ["FEE-5"], "fn": "admit", "base_fee": "10", "size_bytes": size.to_string(), "max_fee": max_fee.to_string(), "expect": expect}));
    }
    json!({"format": 1, "u64": "decimal strings", "spec": "spec/04-fees.md", "generator": "maya-spec-ref spec-vectors", "cases": cases})
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| seed.wrapping_add(u8::try_from(i % 251).expect("below 251")))
        .collect()
}

fn encoding_vectors(k: &BTreeMap<String, Address>) -> Value {
    let signed = Frame {
        outputs: vec![(k["k1"], 42), (resolve(k, FRESH), 7)],
        public_key: pattern(wire::HYBRID_PK, 3),
        nonce: 9,
        signature: Some(pattern(wire::HYBRID_SIG, 101)),
    };
    let unsigned = Frame {
        signature: None,
        ..signed.clone()
    };
    // TX-5: what the frame signs depends on the chain it is meant for; any
    // fixed genesis id shows the layout.
    let chain_tag = [0x5A; 32];
    let good = |id: &str, f: &Frame| {
        json!({"id": id, "rules": ["ENC-1", "ENC-2", "ENC-3", "ENC-4", "ENC-5", "ENC-6", "ENC-7", "ENC-8", "TX-2", "TX-3", "TX-5"], "bytes": hex(&wire::encode(f)),
               "chain_tag": hex(&chain_tag),
               "expect": {"result": "ok", "txid": hex(&wire::txid(f)), "sender": hex(&wire::address_of(&f.public_key)), "nonce": f.nonce.to_string(),
                          "signing_bytes_blake3": hex(blake3::hash(&wire::signing_bytes(f, &chain_tag)).as_bytes())}})
    };
    let bad = |id: &str, rules: &[&str], bytes: Vec<u8>| json!({"id": id, "rules": rules, "bytes": hex(&bytes), "expect": {"result": "error", "error": "Decode"}});
    let full = wire::encode(&signed);
    let mut flag2 = wire::encode(&unsigned);
    *flag2.last_mut().unwrap() = 2;
    let mut trailing = full.clone();
    trailing.push(0);
    let mut too_many = vec![wire::WIRE_VERSION_TRANSFER];
    too_many.extend_from_slice(&0u64.to_le_bytes());
    too_many.extend_from_slice(&(wire::MAX_COLLECTION_LEN + 1).to_le_bytes());
    let mut short_count = vec![wire::WIRE_VERSION_TRANSFER];
    short_count.extend_from_slice(&0u64.to_le_bytes());
    short_count.extend_from_slice(&3u64.to_le_bytes());
    short_count.extend_from_slice(&[0; 40]);
    let cases = vec![
        good("signed-transfer", &signed),
        good("unsigned-transfer", &unsigned),
        bad("single-signature-era-version", &["ENC-1"], vec![4]),
        bad("unknown-version", &["ENC-1"], vec![9]),
        bad("signature-flag-2", &["ENC-4"], flag2),
        bad("trailing-byte", &["ENC-2", "ENC-5"], trailing),
        bad("truncated", &["ENC-5"], full[..full.len() - 1].to_vec()),
        bad("count-above-maximum", &["ENC-6"], too_many),
        bad("count-exceeds-input", &["ENC-3", "ENC-7"], short_count),
    ];
    json!({"format": 1, "u64": "decimal strings", "spec": "spec/01-encoding.md", "generator": "maya-spec-ref spec-vectors", "cases": cases})
}

/// CON-4: the same block with its header declaring the root execution
/// produces (accepted) or another one (refused, nothing written).
fn declared_root_cases(k: &BTreeMap<String, Address>) -> Vec<Value> {
    let pre = json!([acct("k0", 1000, 0), acct("k1", 0, 0)]);
    let block = json!([tx("k0", 0, &[("k1", 10)])]);
    let honest = stf_case(k, "declared-root-matches", &["CON-4"], &pre, &block);
    let root = honest["expect"]["post_root"].clone();
    let mut matches = honest.clone();
    matches["declared_state_root"] = root;
    let mut lies = stf_case(k, "declared-root-mismatch", &["CON-4"], &pre, &block);
    lies["declared_state_root"] = json!(FRESH);
    lies["expect"] = json!({"result": "error", "error": "StateRootMismatch"});
    vec![matches, lies]
}

fn target(lead_zero_bytes: usize, fill: u8) -> [u8; 32] {
    let mut t = [fill; 32];
    t[..lead_zero_bytes].fill(0);
    t
}

fn retarget_cases() -> Vec<Value> {
    let limit = target(4, 0xFF);
    let previous = target(5, 0x7F);
    let expected = consensus::EXPECTED_TIMESPAN;
    let mut cases: Vec<Value> = [
        ("retarget-on-time", expected),
        ("retarget-slow-window", expected * 2),
        ("retarget-fast-window", expected / 2),
        ("retarget-clamped-slow", expected * 10),
        ("retarget-clamped-fast", 1),
    ]
    .iter()
    .map(|(id, span)| {
        let next = consensus::retarget(&previous, *span, &limit);
        json!({"id": id, "rules": ["CON-5"], "fn": "retarget", "previous": hex(&previous), "timespan": span.to_string(), "pow_limit": hex(&limit), "expect": {"result": "ok", "target": hex(&next)}})
    })
    .collect();
    let easy = target(4, 0x80);
    let capped = consensus::retarget(&easy, expected * 4, &limit);
    cases.push(json!({"id": "retarget-capped-at-limit", "rules": ["CON-5"], "fn": "retarget", "previous": hex(&easy), "timespan": (expected * 4).to_string(), "pow_limit": hex(&limit), "expect": {"result": "ok", "target": hex(&capped)}}));
    // A header declaring anything else is invalid.
    let right = consensus::retarget(&previous, expected * 2, &limit);
    let mut wrong = right;
    wrong[31] ^= 1;
    cases.push(json!({"id": "retarget-declared-wrong", "rules": ["CON-5"], "fn": "retarget", "previous": hex(&previous), "timespan": (expected * 2).to_string(), "pow_limit": hex(&limit), "declared": hex(&wrong), "expect": {"result": "error", "error": "WrongTarget"}}));
    cases
}

fn pow_cases() -> Vec<Value> {
    let t = target(2, 0x40);
    let mut equal = t;
    let mut above = t;
    above[2] = 0x41;
    let mut below = t;
    below[31] = 0x3F;
    equal[0] = 0;
    [("pow-below-target", below), ("pow-equal-target", equal), ("pow-above-target", above)]
        .iter()
        .map(|(id, hash)| {
            let expect = if consensus::meets_target(hash, &t) {
                json!({"result": "ok"})
            } else {
                json!({"result": "error", "error": "InsufficientWork"})
            };
            json!({"id": id, "rules": ["CON-6"], "fn": "meets_target", "hash": hex(hash), "target": hex(&t), "expect": expect})
        })
        .collect()
}

fn fork_choice_cases() -> Vec<Value> {
    let hard = target(6, 0xFF);
    let easy = target(4, 0xFF);
    // Three hard blocks against ten easy ones: fewer blocks, more work.
    let a = vec![hard; 3];
    let b = vec![easy; 10];
    let (wa, wb) = (
        consensus::cumulative_work(&a),
        consensus::cumulative_work(&b),
    );
    let branch = |ts: &[[u8; 32]]| ts.iter().map(|t| json!(hex(t))).collect::<Vec<_>>();
    let winner = if wa > wb { "a" } else { "b" };
    let work_case = |id: &str, t: [u8; 32]| json!({"id": id, "rules": ["CON-7"], "fn": "work", "target": hex(&t), "expect": {"result": "ok", "work": hex(&consensus::work(&t))}});
    vec![
        work_case("work-of-a-target", hard),
        work_case("work-of-the-zero-target-saturates", [0; 32]),
        work_case("work-of-the-easiest-target-is-one", [0xFF; 32]),
        json!({"id": "most-work-not-most-blocks", "rules": ["CON-7"], "fn": "fork_choice", "a": branch(&a), "b": branch(&b), "expect": {"result": "ok", "work_a": hex(&wa), "work_b": hex(&wb), "winner": winner}}),
    ]
}

fn prune_cases() -> Vec<Value> {
    [("prune-above-horizon", 101u64, 100u64), ("prune-at-horizon", 100, 100), ("prune-below-horizon", 40, 100)]
        .iter()
        .map(|(id, height, horizon)| {
            let expect = if consensus::below_prune_horizon(*height, *horizon) {
                json!({"result": "error", "error": "BelowPruneHorizon"})
            } else {
                json!({"result": "ok"})
            };
            json!({"id": id, "rules": ["CON-8"], "fn": "prune_horizon", "height": height.to_string(), "horizon": horizon.to_string(), "expect": expect})
        })
        .collect()
}

fn verification_cases() -> Vec<Value> {
    [
        ("v5-verify", 5u8, "verify", true),
        ("v7-verify-needs-height", 7, "verify", true),
        ("v7-verify-at", 7, "verify_at", true),
        ("v7-verify-at-bad-signature", 7, "verify_at", false),
        ("v8-verify-needs-height", 8, "verify", true),
        ("v8-verify-at", 8, "verify_at", true),
    ]
    .iter()
    .map(|(id, version, call, valid)| {
        let expect = match consensus::verification(*version, call, *valid) {
            Ok(()) => json!({"result": "ok"}),
            Err(why) => json!({"result": "error", "error": why}),
        };
        json!({"id": id, "rules": ["TX-4"], "fn": "verification", "version": version, "call": call, "signature": if *valid { "valid" } else { "invalid" }, "height": "0", "expect": expect})
    })
    .collect()
}

fn consensus_vectors() -> Value {
    let cases: Vec<Value> = [
        retarget_cases(),
        pow_cases(),
        fork_choice_cases(),
        prune_cases(),
        verification_cases(),
    ]
    .into_iter()
    .flatten()
    .collect();
    json!({"format": 1, "u64": "decimal strings", "spec": "spec/05-consensus.md, spec/02-transactions.md", "generator": "maya-spec-ref spec-vectors", "cases": cases})
}

fn header_vectors(k: &BTreeMap<String, Address>) -> Value {
    let h = header::Header {
        prev_hash: [0x11; 32],
        state_root: [0x22; 32],
        timestamp: 1_756_252_800,
        nonce: 0x0123_4567_89ab_cdef,
        difficulty_target: [0x0f; 32],
        tx_root: [0x33; 32],
    };
    let frames: Vec<Frame> = (0..3u64)
        .map(|n| Frame {
            outputs: vec![(k["k1"], 5 + n)],
            public_key: pattern(wire::HYBRID_PK, 7),
            nonce: n,
            signature: (n == 0).then(|| pattern(wire::HYBRID_SIG, 9)),
        })
        .collect();
    let txs: Vec<String> = frames.iter().map(|f| hex(&wire::encode(f))).collect();
    let root = header::tx_root(&frames.iter().map(wire::txid).collect::<Vec<_>>());
    let fields = |tx_root: [u8; 32]| json!({"prev_hash": hex(&h.prev_hash), "state_root": hex(&h.state_root), "timestamp": h.timestamp.to_string(), "nonce": h.nonce.to_string(), "difficulty_target": hex(&h.difficulty_target), "tx_root": hex(&tx_root)});
    let cases = vec![
        json!({"id": "header-layout-and-id", "rules": ["CON-1", "CON-2"], "header": fields(h.tx_root), "expect": {"result": "ok", "bytes": hex(&header::serialize(&h)), "id": hex(&header::block_id(&h))}}),
        json!({"id": "tx-root-matches", "rules": ["CON-3", "ROOT-3"], "header": fields(root), "transactions": txs, "expect": {"result": "ok", "tx_root": hex(&root)}}),
        json!({"id": "tx-root-mismatch", "rules": ["CON-3"], "header": fields([0; 32]), "transactions": txs, "expect": {"result": "error", "error": "TxRootMismatch"}}),
    ];
    json!({"format": 1, "u64": "decimal strings", "spec": "spec/05-consensus.md", "generator": "maya-spec-ref spec-vectors", "cases": cases})
}

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let k = keys();
    let outputs = [
        ("state_transitions.json", state_transitions(&k)),
        ("fees.json", fee_vectors()),
        ("encoding.json", encoding_vectors(&k)),
        ("headers.json", header_vectors(&k)),
        ("consensus.json", consensus_vectors()),
    ];
    let mut stale = Vec::new();
    for (name, doc) in outputs {
        let text = serde_json::to_string_pretty(&doc).unwrap() + "\n";
        let path = spec_dir().join(name);
        if check {
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                stale.push(name);
            }
        } else {
            std::fs::write(&path, text).unwrap();
            println!("wrote spec/tests/{name}");
        }
    }
    if !stale.is_empty() {
        eprintln!(
            "spec vectors are stale; run `cargo run -p maya-spec-ref --bin spec-vectors`: {stale:?}"
        );
        std::process::exit(1);
    }
}
