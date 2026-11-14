//! The ledger against the response shapes the node sends, and its refusal to
//! fill a column nothing public can fill.

use std::future::Future;

use maya_sentinel::obs::Transport;
use maya_validator_ledger::{MISSING, build, render_text, to_json};
use serde_json::{Value, json};

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

fn checkpoint(epoch: u64, signers: &[u16]) -> Value {
    let sigs: Vec<Value> = signers.iter().map(|i| json!([i, "ab"])).collect();
    json!({ "epoch": epoch, "height": epoch * 100, "block": "cd".repeat(32), "signatures": sigs })
}

fn node(m: &str, args: &[u64]) -> Result<Value, String> {
    match (m, args) {
        ("get_bft_status", []) => Ok(json!({
            "epoch": 5, "round": 900, "committed_round": 898,
            "validator": null, "committee": 4, "follower": false,
        })),
        ("get_checkpoint", [5]) => Ok(checkpoint(5, &[0, 1, 3])),
        ("get_checkpoint", [4]) => Ok(checkpoint(4, &[0, 1, 2, 3])),
        ("get_checkpoint", [_]) => Ok(Value::Null),
        _ => Err("method not available through the gateway".into()),
    }
}

#[tokio::test]
async fn rows_come_from_checkpoints_oldest_first() {
    let l = build(&Scripted(node), 3).await.expect("built");
    assert_eq!((l.epoch, l.committee, l.committed_round), (5, 4, 898));
    let epochs: Vec<u64> = l.rows.iter().map(|r| r.epoch).collect();
    assert_eq!(epochs, [3, 4, 5]);
    assert!(l.rows[0].checkpoint.is_none(), "epoch 3 not held");
    assert_eq!(
        l.rows[1].checkpoint.as_ref().map(|c| c.signers.len()),
        Some(4)
    );
    // Absence is computed only where the committee size is known.
    assert_eq!(l.rows[1].absent, None);
    assert_eq!(l.rows[2].absent, Some(vec![2]));
}

#[tokio::test]
async fn depth_past_genesis_stops_at_epoch_zero() {
    let l = build(&Scripted(node), 50).await.expect("built");
    assert_eq!(l.rows.first().map(|r| r.epoch), Some(0));
    assert_eq!(l.rows.len(), 6);
}

#[tokio::test]
async fn a_node_without_bft_is_an_error_not_an_empty_ledger() {
    let t = Scripted(|m: &str, _: &[u64]| match m {
        "get_bft_status" => Ok(Value::Null),
        _ => Err("unexpected".into()),
    });
    let err = build(&t, 3).await.expect_err("no BFT, no ledger");
    assert!(err.contains("get_bft_status"), "{err}");
}

#[tokio::test]
async fn every_output_names_what_is_missing() {
    let l = build(&Scripted(node), 2).await.expect("built");
    let text = render_text(&l);
    assert!(text.contains("epoch 5: 3/4 signed, absent [2]"), "{text}");
    assert!(text.contains("epoch 4: 4 signed"), "{text}");
    for m in MISSING {
        assert!(text.contains(m), "text lacks {m}");
    }
    let j = to_json(&l);
    assert_eq!(j["missing"].as_array().map(Vec::len), Some(MISSING.len()));
    assert_eq!(j["epochs"][1]["signers"], json!([0, 1, 3]));
    assert_eq!(j["epochs"][1]["absent"], json!([2]));
    assert!(j["epochs"][0]["absent"].is_null());
    // No stake column exists to be mistaken for a zero.
    assert!(j.get("stake").is_none());
}

#[tokio::test]
async fn a_failed_epoch_is_a_row_and_no_checkpoint_means_no_absence() {
    let t = Scripted(|m: &str, args: &[u64]| match (m, args) {
        ("get_checkpoint", [4]) => Err("429 too many requests".into()),
        ("get_checkpoint", [5]) => Ok(Value::Null),
        _ => node(m, args),
    });
    let l = build(&t, 3).await.expect("one failed epoch does not abort");
    assert_eq!(l.rows.len(), 3);
    assert!(
        l.rows[1]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("429"))
    );
    // The current epoch has no checkpoint: nobody is reported absent.
    assert_eq!(l.rows[2].absent, None);
    assert!(render_text(&l).contains("epoch 4: get_checkpoint [4] failed"));
}
