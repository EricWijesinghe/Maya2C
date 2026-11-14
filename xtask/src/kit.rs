//! `cargo xtask kit` — the developer's testnet kit: one command for a local
//! devnet, its endpoints, and the attack lab.
//!
//! - `kit up` / `kit down` are `up` and `down` (see `up.rs`): four validators,
//!   the gateway, the chat relay, the faucet and the explorer, all on loopback,
//!   with a funded devnet wallet whose keystore path it prints.
//! - `kit status` reads `target/up/up.json` and reports which endpoints answer.
//! - `kit attack` runs attacknet's named attacks against its own throwaway
//!   network. Refused while `kit up` is running, because both use the RPC ports.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;

use crate::attacknet;
use crate::devnet::Result;
use crate::up;

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const USAGE: &str = "\
cargo xtask kit <command>

  up                  Start the local devnet and its endpoints; prints URLs and the wallet path.
  down                Stop what `kit up` started.
  status              Which endpoints of the running devnet answer.
  attack [NAME] [--weighted]
                      Run one attack of the attack lab on its own network
                      (no NAME, or `list`: the names).";

/// One endpoint of the running devnet and whether it answered.
#[derive(Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub url: String,
    pub answering: bool,
}

/// The `host:port` of an `http://host:port[/path]` URL.
pub fn host_port(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    authority.contains(':').then(|| authority.to_owned())
}

/// Every HTTP endpoint in `up.json`, probed by `probe` (a `host:port` in,
/// answering or not out). The chat relay is a libp2p multiaddr, not HTTP, so
/// it is not listed.
pub fn status_rows(state: &Value, probe: &dyn Fn(&str) -> bool) -> Vec<Row> {
    let endpoints = &state["endpoints"];
    let mut named: Vec<(String, String)> = Vec::new();
    for url in endpoints["node_rpc"].as_array().into_iter().flatten() {
        if let Some(url) = url.as_str() {
            named.push((format!("node {}", named.len()), url.to_owned()));
        }
    }
    for key in ["gateway", "faucet", "explorer"] {
        if let Some(url) = endpoints[key].as_str() {
            named.push((key.to_owned(), url.to_owned()));
        }
    }
    named
        .into_iter()
        .map(|(label, url)| {
            let answering = host_port(&url).is_some_and(|hp| probe(&hp));
            Row {
                label,
                url,
                answering,
            }
        })
        .collect()
}

/// Whether something accepts a TCP connection on `addr` within a short wait.
fn probe_tcp(addr: &str) -> bool {
    addr.parse::<SocketAddr>()
        .is_ok_and(|a| TcpStream::connect_timeout(&a, PROBE_TIMEOUT).is_ok())
}

fn status() -> Result<()> {
    let state = up::read_state()?;
    let rows = status_rows(&state, &probe_tcp);
    for row in &rows {
        let mark = if row.answering { "up  " } else { "DOWN" };
        println!("{mark} {:<8} {}", row.label, row.url);
    }
    let down: Vec<&str> = rows
        .iter()
        .filter(|r| !r.answering)
        .map(|r| r.label.as_str())
        .collect();
    if down.is_empty() {
        Ok(())
    } else {
        Err(format!("not answering: {}", down.join(", ")))
    }
}

/// The attacknet arguments for `kit attack REST...`: the scenario name first,
/// then the flags it passes through (`--weighted`).
pub fn attack_args(rest: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(name) = rest.first() {
        args.push("--scenario".to_owned());
        args.push(name.clone());
    }
    args.extend(rest.iter().skip(1).cloned());
    args
}

fn attack(rest: &[String]) -> Result<()> {
    match rest.first().map(String::as_str) {
        None | Some("list") => {
            for name in attacknet::scenario_names() {
                println!("{name}");
            }
            Ok(())
        }
        Some(_) => {
            if up::is_up() {
                return Err(
                    "kit is up: its validators hold the RPC ports the attack lab uses; run `cargo xtask kit down` first"
                        .into(),
                );
            }
            attacknet::attacknet(&attack_args(rest))
        }
    }
}

/// Dispatches `cargo xtask kit ...`.
///
/// # Errors
///
/// An unknown command, or the command's own failure.
pub fn run(args: &[String]) -> Result<()> {
    let Some((cmd, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Err("no kit command given".into());
    };
    match cmd.as_str() {
        "up" => up::up(),
        "down" => up::down(),
        "status" => status(),
        "attack" => attack(rest),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown kit command `{other}`\n{USAGE}")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn state() -> Value {
        json!({
            "endpoints": {
                "node_rpc": ["http://127.0.0.1:32000", "http://127.0.0.1:32001"],
                "gateway": "http://127.0.0.1:8080",
                "chat_relay": "/ip4/127.0.0.1/tcp/4001",
                "faucet": "http://127.0.0.1:8090",
                "explorer": "http://127.0.0.1:3000"
            }
        })
    }

    #[test]
    fn host_port_takes_the_authority_of_an_http_url() {
        assert_eq!(
            host_port("http://127.0.0.1:8080").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(
            host_port("http://127.0.0.1:3000/a/b").as_deref(),
            Some("127.0.0.1:3000")
        );
    }

    #[test]
    fn a_url_without_a_port_or_scheme_is_not_probed() {
        assert_eq!(host_port("https://example.test"), None);
        assert_eq!(host_port("/ip4/127.0.0.1/tcp/4001"), None);
    }

    #[test]
    fn every_http_endpoint_is_probed_and_the_relay_is_not() {
        let probed = std::cell::RefCell::new(Vec::new());
        let rows = status_rows(&state(), &|hp| {
            probed.borrow_mut().push(hp.to_owned());
            true
        });
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|r| r.answering));
        assert!(!probed.borrow().iter().any(|p| p.contains("4001")));
    }

    #[test]
    fn a_service_that_does_not_answer_is_reported_by_its_label() {
        let rows = status_rows(&state(), &|hp| !hp.ends_with(":8090"));
        let faucet = rows.iter().find(|r| r.label == "faucet").unwrap();
        assert!(!faucet.answering);
        let gateway = rows.iter().find(|r| r.label == "gateway").unwrap();
        assert!(gateway.answering);
    }

    #[test]
    fn a_state_without_endpoints_has_no_rows() {
        assert!(status_rows(&json!({}), &|_| true).is_empty());
    }

    #[test]
    fn attack_args_put_the_scenario_first_and_keep_flags() {
        let rest = vec!["stolen-key".to_owned(), "--weighted".to_owned()];
        assert_eq!(
            attack_args(&rest),
            ["--scenario", "stolen-key", "--weighted"]
        );
    }

    #[test]
    fn an_unknown_kit_command_is_refused() {
        assert!(run(&["bogus".to_owned()]).is_err());
    }
}
