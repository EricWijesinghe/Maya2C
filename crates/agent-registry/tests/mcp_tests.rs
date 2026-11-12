mod common;

use common::{NOW, signed_at, signer};
use maya_agent_registry::mcp;
use serde_json::{Value, json};

#[test]
fn the_tool_surface_names_four_tools() {
    let names: Vec<String> = mcp::tools()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        names,
        [
            "agent_search",
            "agent_verify_card",
            "agent_draft_payment",
            "agent_verify_receipt"
        ]
    );
}

#[test]
fn a_tool_outside_this_surface_is_not_claimed() {
    assert!(mcp::call("get_height", &json!({})).is_none());
}

#[test]
fn search_refuses_a_registry_that_is_not_loopback() {
    let args = json!({"registry": "http://example.com:80", "capability": "translate"});
    let result = mcp::call("agent_search", &args).expect("claimed");
    assert!(result.is_err());
}

#[test]
fn search_refuses_a_capability_that_would_need_escaping() {
    let args = json!({"registry": "http://127.0.0.1:1", "capability": "a b&c"});
    let result = mcp::call("agent_search", &args).expect("claimed");
    assert!(result.is_err());
}

#[test]
fn verify_card_accepts_a_valid_card_and_refuses_a_tampered_one() {
    let alice = signer(1);
    let card = signed_at(&alice, "translate", NOW);
    let good = mcp::call("agent_verify_card", &json!({"card": card})).expect("claimed");
    let good: Value = good.expect("valid card");
    assert_eq!(good["valid"], json!(true));

    let mut bad = serde_json::to_value(&card).expect("json");
    bad["card"]["name"] = json!("impostor");
    let result = mcp::call("agent_verify_card", &json!({"card": bad})).expect("claimed");
    assert!(result.is_err());
}
