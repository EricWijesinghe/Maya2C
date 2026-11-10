//! `maya2c-checkout`: the checkout service process.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use maya_address::Network;
use maya_checkout::chain::{ChainSource, RpcChain};
use maya_checkout::config::Config;
use maya_checkout::engine::{self, HttpSender};
use maya_checkout::http::{AppState, router};
use maya_checkout::store::Store;
use maya_checkout::{CheckoutError, Result};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "checkout service stopped");
            ExitCode::FAILURE
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

async fn run() -> Result<()> {
    let cfg = Config::from_lookup(|k| std::env::var(k).ok())?;
    tracing::info!(config = ?cfg, "configuration loaded");
    let chain: Arc<dyn ChainSource> = Arc::new(RpcChain::new(&cfg.rpc_url)?);
    let tip = chain.tip().await?;
    let network = tip
        .chain_id
        .as_deref()
        .and_then(Network::for_chain_id)
        .ok_or_else(|| CheckoutError::Config("node reports no known network".into()))?;
    let (pay_network, pay_to) = maya_address::decode_any(&cfg.pay_to)
        .map_err(|e| CheckoutError::Config(format!("MAYA_CHECKOUT_PAY_TO: {e}")))?;
    if pay_network != network {
        return Err(CheckoutError::Config(
            "MAYA_CHECKOUT_PAY_TO is for a different network than the node".into(),
        ));
    }
    let mut store = Store::open(&cfg.store_path)?;
    store.ledger_mut().scanned_to.get_or_insert(tip.height);
    store.commit()?;
    let store = Arc::new(Mutex::new(store));

    let secret = cfg.webhook_secret.clone();
    let scanner_store = Arc::clone(&store);
    let scanner_chain = Arc::clone(&chain);
    let (confirmations, poll) = (cfg.confirmations, Duration::from_secs(cfg.poll_secs));
    tokio::spawn(async move {
        loop {
            let sender = HttpSender;
            if let Err(e) = engine::run_once(
                &scanner_store,
                scanner_chain.as_ref(),
                &sender,
                &secret,
                confirmations,
                unix_now(),
            )
            .await
            {
                tracing::warn!(error = %e, "scan cycle failed; retrying");
            }
            tokio::time::sleep(poll).await;
        }
    });

    let state = Arc::new(AppState {
        store,
        chain,
        network,
        pay_to,
        api_token: cfg.api_token.clone(),
    });
    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .map_err(|e| CheckoutError::Config(format!("cannot listen on {}: {e}", cfg.listen)))?;
    tracing::info!(listen = %cfg.listen, "serving");
    axum::serve(listener, router(state))
        .await
        .map_err(|e| CheckoutError::Internal(format!("server: {e}")))
}
