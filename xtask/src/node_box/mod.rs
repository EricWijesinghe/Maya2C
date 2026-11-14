//! `cargo xtask node` — node-in-a-box: a guided setup and a health check for a
//! Maya2C node on this machine. Lives in xtask beside `up`, `localnet` and the
//! other operator tooling, not in `maya2c-cli`, which is the developer CLI and
//! links the whole node.
//!
//! - `node setup --genesis FILE [--dir DIR]`: host checks (disk, RAM, ports,
//!   clock), then `node-config.toml` from the node's own template, the genesis
//!   copied in, key files secured, and the service definition printed. It
//!   installs nothing.
//! - `node doctor [--dir DIR]`: config, genesis, key permissions, the node's
//!   RPC, and the host checks again. Exits nonzero on any FAIL.
//!
//! Per-OS steps: docs/ecosystem/node-in-a-box-windows.md and -linux.md.

mod acl;
mod checks;
mod config;
mod service;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::devnet::{self, Result};
use acl::Os;
use checks::{Check, Level};
use config::Written;

/// The node's own defaults (`maya2c-node --help`).
const DEFAULT_P2P_PORT: u16 = 30_333;
const DEFAULT_RPC_PORT: u16 = 8_545;
/// Key files setup secures when present. A passphrase file is a secret too.
const KEY_FILES: [&str; 3] = [
    "validator.key",
    "validator.key.sealed",
    "validator.passphrase",
];
const USAGE: &str = "\
cargo xtask node <command>

  setup --genesis FILE [--dir DIR] [--p2p-port N] [--rpc-port N] [--force]
                        Check the host, write node-config.toml, copy genesis in,
                        secure key files, print the service definition (not installed).
  doctor [--dir DIR] [--rpc-port N]
                        Health check: config, genesis, key permissions, the node's
                        RPC, and host checks. Nonzero exit on any FAIL.

  DIR defaults to ~/maya2c-node (USERPROFILE on Windows). Build the node first:
  cargo build -p maya2c-node (or point MAYA2C_BIN_DIR at built binaries).";

#[derive(Debug, PartialEq, Eq)]
struct Opts {
    dir: PathBuf,
    genesis: Option<PathBuf>,
    p2p_port: u16,
    rpc_port: u16,
    force: bool,
}

fn default_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("maya2c-node")
}

fn value<'a>(it: &mut std::slice::Iter<'a, String>, flag: &str) -> Result<&'a String> {
    it.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_opts(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        dir: default_dir(),
        genesis: None,
        p2p_port: DEFAULT_P2P_PORT,
        rpc_port: DEFAULT_RPC_PORT,
        force: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dir" => o.dir = PathBuf::from(value(&mut it, arg)?),
            "--genesis" => o.genesis = Some(PathBuf::from(value(&mut it, arg)?)),
            "--p2p-port" => {
                o.p2p_port = value(&mut it, arg)?
                    .parse()
                    .map_err(|e| format!("--p2p-port: {e}"))?;
            }
            "--rpc-port" => {
                o.rpc_port = value(&mut it, arg)?
                    .parse()
                    .map_err(|e| format!("--rpc-port: {e}"))?;
            }
            "--force" => o.force = true,
            other => return Err(format!("unknown option `{other}`\n{USAGE}")),
        }
    }
    Ok(o)
}

fn user() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "operator".to_owned())
}

fn key_files(dir: &Path) -> Vec<PathBuf> {
    KEY_FILES
        .iter()
        .map(|name| dir.join(name))
        .filter(|p| p.exists())
        .collect()
}

fn check(name: &'static str, level: Level, detail: impl Into<String>) -> Check {
    Check {
        name,
        level,
        detail: detail.into(),
    }
}

fn print_checks(checks: &[Check]) {
    for c in checks {
        let tag = match c.level {
            Level::Pass => "PASS",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        };
        println!("{tag} {:<7} {}", c.name, c.detail);
    }
}

/// Whether any check is a FAIL. Warnings do not stop setup.
fn any_fail(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.level == Level::Fail)
}

/// Copies `--genesis` into the node directory (when it is not already there)
/// and returns the genesis's SHA-256, to compare with the seed operator's.
fn place_genesis(o: &Opts) -> Result<String> {
    let dest = o.dir.join("genesis.json");
    if let Some(src) = o.genesis.as_ref().filter(|s| **s != dest) {
        std::fs::copy(src, &dest).map_err(|e| format!("copying genesis: {e}"))?;
    }
    let bytes = std::fs::read(&dest).map_err(|_| {
        "no genesis.json: pass --genesis FILE (the network's published genesis)".to_owned()
    })?;
    Ok(format!("sha256 {}", hex::encode(Sha256::digest(&bytes))))
}

/// The node's own config template, so it matches the node's defaults.
fn node_template() -> Result<String> {
    let exe = devnet::bin("maya2c-node");
    if !exe.exists() {
        return Err(format!(
            "{} is not built: run `cargo build -p maya2c-node` first",
            exe.display()
        ));
    }
    devnet::output(Command::new(&exe).arg("--print-config-template"))
}

fn secure_keys(dir: &Path, os: Os, user: &str) -> Result<Vec<Check>> {
    let mut out = Vec::new();
    for key in key_files(dir) {
        let shown = key.display().to_string();
        for step in acl::key_acl_steps(os, &shown, user) {
            acl::run_step(&step)?;
        }
        let left = acl::current_violations(os, &key, user)?;
        out.push(if left.is_empty() {
            check("keys", Level::Pass, format!("{shown}: only you and SYSTEM"))
        } else {
            check(
                "keys",
                Level::Warn,
                format!(
                    "{shown} still open to {}; remove those grants by hand",
                    left.join(", ")
                ),
            )
        });
    }
    Ok(out)
}

fn setup(args: &[String]) -> Result<()> {
    let o = parse_opts(args)?;
    std::fs::create_dir_all(o.dir.join("data")).map_err(|e| format!("{}: {e}", o.dir.display()))?;
    println!("host:");
    let host = checks::evaluate(&checks::gather(&o.dir, Some(&[o.p2p_port, o.rpc_port])));
    print_checks(&host);
    if any_fail(&host) {
        return Err("host checks failed; fix the FAIL lines and run setup again".into());
    }
    println!("genesis: {}", place_genesis(&o)?);
    let cfg = o.dir.join("node-config.toml");
    match config::write_config(&cfg, &node_template()?, o.force)? {
        Written::Created => println!("config: wrote {}", cfg.display()),
        Written::Replaced => println!("config: replaced {} (--force)", cfg.display()),
        Written::Kept => println!(
            "config: kept existing {} (--force to replace)",
            cfg.display()
        ),
    }
    let os = Os::current();
    let user = user();
    print_checks(&secure_keys(&o.dir, os, &user)?);
    let validator = o.dir.join("validator.key");
    let args = service::node_args(&o.dir, validator.exists().then_some(validator.as_path()));
    let exe = devnet::bin("maya2c-node");
    println!("\nservice (printed, not installed; apply it yourself):");
    match os {
        Os::Windows => println!("{}", service::windows_task_command(&exe, &args)),
        Os::Unix => println!("{}", service::systemd_unit(&exe, &args, &user)),
    }
    println!("\nnext: cargo xtask node doctor --dir {}", o.dir.display());
    Ok(())
}

fn config_check(dir: &Path) -> Check {
    let path = dir.join("node-config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return check(
            "config",
            Level::Fail,
            "no node-config.toml; run `node setup`",
        );
    };
    match config::missing_sections(&text) {
        Ok(missing) if missing.is_empty() => check("config", Level::Pass, "all sections present"),
        Ok(missing) => check(
            "config",
            Level::Fail,
            format!("missing section(s): {}", missing.join(", ")),
        ),
        Err(e) => check("config", Level::Fail, e),
    }
}

fn genesis_check(dir: &Path) -> Check {
    match std::fs::read(dir.join("genesis.json")) {
        Ok(bytes) => check(
            "genesis",
            Level::Pass,
            format!("sha256 {}", hex::encode(Sha256::digest(&bytes))),
        ),
        Err(_) => check(
            "genesis",
            Level::Fail,
            "no genesis.json; pass --genesis to setup",
        ),
    }
}

fn key_checks(dir: &Path) -> Vec<Check> {
    let keys = key_files(dir);
    if keys.is_empty() {
        return vec![check(
            "keys",
            Level::Warn,
            "no validator key here (observer only)",
        )];
    }
    let (os, user) = (Os::current(), user());
    keys.iter()
        .map(|key| match acl::current_violations(os, key, &user) {
            Ok(v) if v.is_empty() => check(
                "keys",
                Level::Pass,
                format!("{}: only you and SYSTEM", key.display()),
            ),
            Ok(v) => check(
                "keys",
                Level::Fail,
                format!("{} open to {}", key.display(), v.join(", ")),
            ),
            Err(e) => check("keys", Level::Fail, e),
        })
        .collect()
}

fn rpc_check(port: u16) -> Check {
    let addr = format!("127.0.0.1:{port}");
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "get_chain_info", "params": [] });
    match devnet::post(&addr, "/", &request) {
        Err(e) => check("rpc", Level::Fail, format!("not answering on {addr}: {e}")),
        Ok((_, reply)) => match (reply.get("error"), reply["result"]["height"].as_u64()) {
            (Some(error), _) => check("rpc", Level::Fail, format!("{addr}: {error}")),
            (None, Some(h)) => check("rpc", Level::Pass, format!("height {h} on {addr}")),
            (None, None) => check(
                "rpc",
                Level::Fail,
                format!("no height in reply from {addr}"),
            ),
        },
    }
}

fn doctor(args: &[String]) -> Result<()> {
    let o = parse_opts(args)?;
    let mut report = vec![config_check(&o.dir), genesis_check(&o.dir)];
    report.extend(key_checks(&o.dir));
    report.push(rpc_check(o.rpc_port));
    report.extend(checks::evaluate(&checks::gather(&o.dir, None)));
    print_checks(&report);
    let failed = report.iter().filter(|c| c.level == Level::Fail).count();
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} check(s) failed"))
    }
}

/// Dispatches `cargo xtask node ...`.
///
/// # Errors
///
/// An unknown command or option, a failed host check, or a failed doctor check.
pub fn run(args: &[String]) -> Result<()> {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "setup" => setup(rest),
        Some((cmd, rest)) if cmd == "doctor" => doctor(rest),
        Some((cmd, _)) if cmd == "help" || cmd == "--help" || cmd == "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        Some((cmd, _)) => Err(format!("unknown node command `{cmd}`\n{USAGE}")),
        None => {
            println!("{USAGE}");
            Err("no node command given".into())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_defaults_are_the_nodes_own_ports() {
        let o = parse_opts(&[]).unwrap();
        assert_eq!(o.p2p_port, 30_333);
        assert_eq!(o.rpc_port, 8_545);
        assert!(!o.force);
        assert_eq!(o.genesis, None);
    }

    #[test]
    fn every_option_is_read() {
        let o = parse_opts(&args(&[
            "--dir",
            "/srv/n",
            "--genesis",
            "g.json",
            "--p2p-port",
            "31000",
            "--rpc-port",
            "9000",
            "--force",
        ]))
        .unwrap();
        assert_eq!(o.dir, PathBuf::from("/srv/n"));
        assert_eq!(o.genesis, Some(PathBuf::from("g.json")));
        assert_eq!((o.p2p_port, o.rpc_port, o.force), (31_000, 9_000, true));
    }

    #[test]
    fn a_flag_without_its_value_or_an_unknown_flag_is_refused() {
        assert!(parse_opts(&args(&["--dir"])).is_err());
        assert!(parse_opts(&args(&["--p2p-port", "99999"])).is_err());
        assert!(parse_opts(&args(&["--frobnicate"])).is_err());
    }

    #[test]
    fn one_failed_host_check_stops_setup() {
        let ok = check("disk", Level::Pass, "x");
        let bad = check("ram", Level::Fail, "y");
        let warn = check("clock", Level::Warn, "z");
        assert!(any_fail(&[ok, bad]));
        assert!(!any_fail(&[check("clock", Level::Warn, "z"), warn]));
    }

    #[test]
    fn an_unknown_node_command_is_refused() {
        assert!(run(&args(&["launch"])).is_err());
    }

    #[test]
    fn a_missing_genesis_is_named_in_the_error() {
        let dir = std::env::temp_dir().join(format!("xtask-node-nogenesis-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("genesis.json"));
        let opts = Opts {
            dir: dir.clone(),
            genesis: None,
            p2p_port: 1,
            rpc_port: 1,
            force: false,
        };
        let err = place_genesis(&opts).unwrap_err();
        assert!(err.contains("--genesis"));
    }
}
