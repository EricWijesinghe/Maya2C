//! The public checks against scripted polls: each one must fire on the
//! violation it names and stay quiet on a healthy chain.

use std::future::Future;

use maya_invariant_monitor::checks::PublicChecks;
use maya_invariant_monitor::public::{
    Checkpoint, FinalBlock, PublicObs, observe_public, parse_checkpoint,
};
use maya_sentinel::obs::{NodeObs, Sample, Transport};
use serde_json::{Value, json};

fn sample(name: &str, height: u64) -> Sample {
    Sample {
        name: name.into(),
        state: Ok(NodeObs {
            height,
            tip_id: "aa".repeat(32),
            tip_txs: 0,
            peers: None,
            supply: None,
            pending: None,
        }),
    }
}

fn public(
    name: &str,
    height: u64,
    block: &str,
    served: &str,
    bytes: u64,
) -> (String, Result<PublicObs, String>) {
    (
        name.into(),
        Ok(PublicObs {
            checkpoint: Some(Checkpoint {
                epoch: height / 100,
                height,
                block: block.into(),
                signers: vec![0, 1, 2],
            }),
            block: Some(FinalBlock {
                id: served.into(),
                bytes,
            }),
            block_error: None,
        }),
    )
}

fn keys(findings: &[maya_sentinel::detect::Finding]) -> Vec<String> {
    findings.iter().map(|f| f.key.clone()).collect()
}

#[test]
fn a_healthy_chain_raises_nothing() {
    let mut c = PublicChecks::new();
    let a = c.evaluate(
        &[sample("a", 10), sample("b", 10)],
        &[public("a", 8, "x", "x", 500), public("b", 8, "x", "x", 500)],
    );
    let b = c.evaluate(
        &[sample("a", 11), sample("b", 12)],
        &[public("a", 9, "y", "y", 500), public("b", 9, "y", "y", 500)],
    );
    assert!(a.is_empty() && b.is_empty(), "{a:?} {b:?}");
}

#[test]
fn a_height_that_goes_backwards_is_reported() {
    let mut c = PublicChecks::new();
    c.evaluate(&[sample("a", 10)], &[]);
    assert_eq!(
        keys(&c.evaluate(&[sample("a", 7)], &[])),
        ["height-regress:a"]
    );
}

#[test]
fn a_checkpoint_that_goes_backwards_is_reported() {
    let mut c = PublicChecks::new();
    c.evaluate(&[], &[public("a", 300, "x", "x", 1)]);
    assert_eq!(
        keys(&c.evaluate(&[], &[public("a", 200, "w", "w", 1)])),
        ["checkpoint-regress:a"]
    );
}

#[test]
fn two_blocks_at_one_finalized_height_is_a_revert() {
    let mut c = PublicChecks::new();
    c.evaluate(&[], &[public("a", 8, "x", "x", 1)]);
    // Another source attests a different block at the same height.
    let f = c.evaluate(&[], &[public("b", 8, "z", "z", 1)]);
    assert_eq!(keys(&f), ["finality-revert:8"]);
    assert_eq!(f[0].severity, maya_sentinel::detect::Severity::Critical);
}

#[test]
fn a_source_serving_a_block_other_than_its_checkpoint_is_a_revert() {
    let mut c = PublicChecks::new();
    assert_eq!(
        keys(&c.evaluate(&[], &[public("a", 8, "x", "q", 1)])),
        ["finality-revert:8"]
    );
}

#[test]
fn a_finalized_block_over_the_cap_is_reported() {
    let cap = maya_fee_market::limits::MAX_BLOCK_BYTES;
    let mut c = PublicChecks::new();
    assert!(c.evaluate(&[], &[public("a", 8, "x", "x", cap)]).is_empty());
    assert_eq!(
        keys(&c.evaluate(&[], &[public("a", 9, "y", "y", cap + 1)])),
        ["block-cap:9"]
    );
}

#[test]
fn the_finalized_window_is_bounded() {
    let mut c = PublicChecks::new();
    let keep = maya_invariant_monitor::checks::FINAL_KEEP as u64;
    for h in 1..=keep + 10 {
        c.evaluate(&[], &[public("a", h, "x", "x", 1)]);
    }
    assert_eq!(
        c.finalized_len(),
        maya_invariant_monitor::checks::FINAL_KEEP
    );
}

struct Scripted<F>(F);

impl<F> Transport for Scripted<F>
where
    F: Fn(&str, &[u64]) -> Result<Value, String> + Send + Sync,
{
    fn call(
        &self,
        method: &'static str,
        args: Vec<u64>,
    ) -> impl Future<Output = Result<Value, String>> + Send {
        let answer = (self.0)(method, &args);
        async move { answer }
    }
}

#[tokio::test]
async fn reads_the_checkpoint_and_its_block_in_the_shapes_the_node_sends() {
    let t = Scripted(|m: &str, args: &[u64]| match (m, args) {
        ("get_checkpoint", []) => Ok(json!({
            "epoch": 3, "height": 812, "block": "cd".repeat(32),
            "signatures": [[0, "aa"], [2, "bb"], [3, "cc"]],
        })),
        ("get_block_by_height", [812]) => Ok(json!({
            "height": 812, "header": { "id": "cd".repeat(32) }, "transactions": [], "raw": "00ff00ff",
        })),
        _ => Err("method not found".into()),
    });
    let o = observe_public(&t).await.expect("observed");
    let cp = o.checkpoint.expect("checkpoint");
    assert_eq!((cp.epoch, cp.height, cp.signers), (3, 812, vec![0, 2, 3]));
    assert_eq!(
        o.block,
        Some(FinalBlock {
            id: "cd".repeat(32),
            bytes: 4
        })
    );
}

#[tokio::test]
async fn a_source_without_a_checkpoint_is_empty_not_an_error() {
    let t = Scripted(|m: &str, _: &[u64]| match m {
        "get_checkpoint" => Ok(Value::Null),
        _ => Err("unexpected".into()),
    });
    assert_eq!(observe_public(&t).await, Ok(PublicObs::default()));
    let down = Scripted(|_: &str, _: &[u64]| Err("connection refused".to_owned()));
    assert!(observe_public(&down).await.is_err());
}

#[test]
fn a_malformed_signer_index_drops_the_checkpoint() {
    let v = json!({ "epoch": 1, "height": 2, "block": "ab", "signatures": [[70000, "aa"]] });
    assert_eq!(parse_checkpoint(&v), None);
}

#[tokio::test]
async fn a_finalized_block_the_source_will_not_serve_is_reported() {
    let t = Scripted(|m: &str, _: &[u64]| match m {
        "get_checkpoint" => Ok(json!({ "epoch": 1, "height": 9, "block": "ab", "signatures": [] })),
        _ => Err("block pruned".into()),
    });
    let o = observe_public(&t).await.expect("observed");
    assert_eq!(o.block, None);
    let mut c = PublicChecks::new();
    assert_eq!(
        keys(&c.evaluate(&[], &[("a".into(), Ok(o))])),
        ["finalized-block-unreadable:a"]
    );
}
