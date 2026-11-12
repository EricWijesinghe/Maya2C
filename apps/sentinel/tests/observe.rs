//! `observe` against the response shapes the node actually sends
//! (`get_chain_info`, `get_block_by_height`, `get_supply`, `get_peers`), with
//! optional calls failing the way they do on a node that lacks them.

use std::future::Future;

use maya_sentinel::obs::{ObserveError, Transport, observe};
use primitive_types::U256;
use serde_json::{Value, json};

/// Answers from a closure; `Err` plays an unreachable or unsupported method.
struct Scripted<F>(F);

impl<F> Transport for Scripted<F>
where
    F: Fn(&str) -> Result<Value, String> + Send + Sync,
{
    fn call(
        &self,
        method: &'static str,
        _args: Vec<u64>,
    ) -> impl Future<Output = Result<Value, String>> + Send {
        let answer = (self.0)(method);
        async move { answer }
    }
}

fn healthy(method: &str) -> Result<Value, String> {
    match method {
        "get_chain_info" => {
            Ok(json!({ "genesis": "00", "chain_id": "maya-testnet-1", "height": 42 }))
        }
        "get_block_by_height" => Ok(json!({
            "height": 42,
            "header": { "id": "ab".repeat(32), "timestamp": 1 },
            "transactions": [{}, {}],
            "raw": "00",
        })),
        "get_supply" => Ok(json!({
            "total": "1000000000000000000000",
            "circulating": "700000000000000000000",
            "shielded": "0",
            "burned": "100000000000000000000",
            "unissued": "200000000000000000000",
        })),
        "get_peers" => Ok(json!({ "peer_id": "p", "peers": [{}, {}, {}, {}] })),
        "get_mempool_info" => Ok(json!({ "pending": 7 })),
        other => Err(format!("{other}: method not found")),
    }
}

#[tokio::test]
async fn reads_every_field_when_the_node_serves_them() {
    let obs = observe(&Scripted(healthy), true).await.expect("observed");
    assert_eq!(obs.height, 42);
    assert_eq!(obs.tip_id, "ab".repeat(32));
    assert_eq!(obs.tip_txs, 2);
    assert_eq!(obs.peers, Some(4));
    assert_eq!(obs.pending, Some(7));
    let s = obs.supply.expect("supply requested");
    assert_eq!(s.total, U256::from(10u64).pow(U256::from(21u64)));
    assert_eq!(s.burned, U256::from(10u64).pow(U256::from(20u64)));
    assert_eq!(
        s.unissued,
        U256::from(2u64) * U256::from(10u64).pow(U256::from(20u64))
    );
    assert!(s.adds_up());
}

#[tokio::test]
async fn a_node_without_the_unissued_field_still_parses() {
    // Nodes from before the field report `burned` with the reserve in it, so
    // an absent `unissued` is zero, not an error.
    let answers = |m: &str| match m {
        "get_supply" => Ok(json!({
            "total": "1000",
            "circulating": "900",
            "shielded": "0",
            "burned": "100",
        })),
        other => healthy(other),
    };
    let s = observe(&Scripted(answers), true)
        .await
        .expect("observed")
        .supply
        .expect("supply parsed");
    assert_eq!(s.unissued, U256::zero());
    assert!(s.adds_up());
}

#[tokio::test]
async fn optional_calls_degrade_to_none_and_never_fail_the_poll() {
    let answers = |m: &str| match m {
        "get_mempool_info" | "get_peers" | "get_supply" => Err("method not found".to_owned()),
        other => healthy(other),
    };
    let obs = observe(&Scripted(answers), true)
        .await
        .expect("still observed");
    assert_eq!(obs.height, 42);
    assert_eq!(obs.pending, None);
    assert_eq!(obs.peers, None);
    assert_eq!(obs.supply, None);
}

#[tokio::test]
async fn supply_is_not_read_unless_asked() {
    let obs = observe(&Scripted(healthy), false).await.expect("observed");
    assert_eq!(obs.supply, None);
}

#[tokio::test]
async fn an_unreachable_node_is_a_call_error() {
    let down = |_: &str| Err("connection refused".to_owned());
    let err = observe(&Scripted(down), false).await.expect_err("down");
    assert_eq!(
        err,
        ObserveError::Call {
            method: "get_chain_info",
            reason: "connection refused".into()
        }
    );
}

#[tokio::test]
async fn a_block_without_its_id_is_a_shape_error_not_a_guess() {
    let no_id = |m: &str| match m {
        "get_block_by_height" => Ok(json!({ "height": 42, "header": {}, "transactions": [] })),
        other => healthy(other),
    };
    let err = observe(&Scripted(no_id), false).await.expect_err("no id");
    assert_eq!(
        err,
        ObserveError::Shape {
            method: "get_block_by_height",
            field: "header.id"
        }
    );
}
