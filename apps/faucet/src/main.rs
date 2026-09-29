//! The faucet daemon.
//!
//! # The key comes from the environment, never from a flag
//!
//! A command line is world-readable on most systems — `/proc/<pid>/cmdline`,
//! `ps`, a shell history file — so a funded signing key passed as `--key` is a
//! key handed to every other process on the host. It is read from
//! `MAYA_FAUCET_KEY` instead, and the process fails to start without it.
//!
//! # It will not start on a chain where value is real
//!
//! `Faucet::new` refuses, and this binary reports that and exits non-zero
//! rather than serving. See `apps/faucet/src/lib.rs`.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::SystemTime;

use custom_l1_node::crypto::hybrid::{HYBRID_SECRET_KEY_LEN, HybridSigningKey};
use maya_faucet::Faucet;
use maya_faucet::dispense::NodeDispenser;
use maya_faucet::http::{FaucetService, router};

/// Environment variable holding the hex-encoded faucet signing key.
const KEY_VAR: &str = "MAYA_FAUCET_KEY";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    // `maya-faucet generate-key <PATH>`: a fresh key, written where only its
    // owner can read it, in the form systemd's EnvironmentFile loads. Only the
    // address is printed: it is what the genesis funds.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("generate-key") {
        return match argv.get(2) {
            Some(path) => match run_on_big_stack(path.clone()) {
                Ok(address) => {
                    println!("{address}");
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("faucet: {message}");
                    ExitCode::FAILURE
                }
            },
            None => {
                eprintln!("usage: maya-faucet generate-key <PATH>");
                ExitCode::FAILURE
            }
        };
    }

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("faucet: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let chain_id = env_or("MAYA_FAUCET_CHAIN", "maya-genesis-rc1");
    let node_url = env_or("MAYA_FAUCET_NODE", "http://127.0.0.1:8545");
    let listen: SocketAddr = env_or("MAYA_FAUCET_LISTEN", "127.0.0.1:8080")
        .parse()
        .map_err(|e| format!("MAYA_FAUCET_LISTEN is not an address: {e}"))?;
    let dispense = parse_env("MAYA_FAUCET_DISPENSE", 1_000)?;
    let daily_cap = parse_env("MAYA_FAUCET_DAILY_CAP", 1_000_000)?;
    let trust_proxy = env_or("MAYA_FAUCET_TRUST_PROXY", "false") == "true";

    let key = load_key()?;

    // Before anything is bound or connected: a value-bearing chain is a
    // configuration error, and the operator should see it now.
    let faucet =
        Faucet::new(chain_id, dispense, daily_cap, SystemTime::now()).map_err(|e| e.to_string())?;

    let dispenser = NodeDispenser::connect(&node_url, key)
        .await
        .map_err(|e| format!("{e}"))?;

    tracing::info!(
        chain_id = faucet.chain_id(),
        address = %hex::encode(dispenser.address()),
        next_nonce = dispenser.next_nonce(),
        dispense,
        daily_cap,
        trust_proxy,
        "faucet ready"
    );
    if !trust_proxy {
        tracing::info!(
            "keying rate limits on the peer address; set MAYA_FAUCET_TRUST_PROXY=true only \
             when a reverse proxy is the sole route to this port"
        );
    }

    let service =
        Arc::new(FaucetService::new(faucet, Arc::new(dispenser)).trust_proxy(trust_proxy));

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("binding {listen}: {e}"))?;
    tracing::info!(%listen, "listening");

    // `into_make_service_with_connect_info` is what puts the peer address in
    // the request extensions. Without it every request would look address-less
    // and the limiter would have nothing to key on — which the extractor
    // refuses rather than serving unlimited.
    axum::serve(
        listener,
        router(service).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| format!("serving: {e}"))
}

/// Hybrid key generation (ML-DSA + SLH-DSA) needs more stack than a debug
/// build gets on Windows' 1 MiB main thread; it overflowed there.
const KEYGEN_STACK: usize = 16 << 20;

fn run_on_big_stack(path: String) -> Result<String, String> {
    std::thread::Builder::new()
        .stack_size(KEYGEN_STACK)
        .spawn(move || generate_key(std::path::Path::new(&path)))
        .map_err(|e| format!("starting key generation: {e}"))?
        .join()
        .map_err(|_| "key generation panicked".to_string())?
}

/// Writes `MAYA_FAUCET_KEY=<hex>` to `path` (mode 0600, never overwriting)
/// and returns the key's address, hex.
fn generate_key(path: &std::path::Path) -> Result<String, String> {
    use std::io::Write as _;
    let key = custom_l1_node::crypto::hybrid::generate_signing_key().map_err(|e| e.to_string())?;
    let line = zeroize::Zeroizing::new(format!(
        "{KEY_VAR}={}
",
        hex::encode(key.to_bytes().as_slice())
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("{}: {e} (refusing to overwrite a key)", path.display()))?;
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(hex::encode(key.address()))
}

/// Reads the signing key from the environment.
fn load_key() -> Result<HybridSigningKey, String> {
    let encoded = std::env::var(KEY_VAR).map_err(|_| {
        format!(
            "{KEY_VAR} is not set. The faucet needs a funded signing key, and it is read from \
             the environment rather than a flag because a command line is readable by every \
             process on the host."
        )
    })?;

    let bytes = hex::decode(encoded.trim()).map_err(|_| format!("{KEY_VAR} is not hex"))?;
    let bytes: [u8; HYBRID_SECRET_KEY_LEN] = bytes
        .try_into()
        .map_err(|_: Vec<u8>| format!("{KEY_VAR} must be {HYBRID_SECRET_KEY_LEN} bytes of hex"))?;

    HybridSigningKey::from_bytes(&bytes).map_err(|e| format!("{KEY_VAR} is not a signing key: {e}"))
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env(name: &str, default: u64) -> Result<u64, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|e| format!("{name} is not a number: {e}")),
        Err(_) => Ok(default),
    }
}
