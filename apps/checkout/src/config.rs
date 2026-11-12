//! Startup configuration from environment variables.
//!
//! Secrets (the API token and the webhook signing key) are read into zeroizing
//! buffers, must be at least 256 bits, must differ from each other, and are
//! never printed: the manual `Debug` below writes only their lengths.

use std::net::SocketAddr;
use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::error::{CheckoutError, Result};
use crate::store::RETAIN_BLOCKS;
use crate::webhook::MIN_SECRET_BYTES;

/// Default listen address: loopback only. Put a TLS proxy in front to expose it.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
/// Default confirmation depth before a payment is reported. Shops may raise it.
pub const DEFAULT_CONFIRMATIONS: u64 = 6;
/// Default pause between scan cycles, in seconds.
pub const DEFAULT_POLL_SECS: u64 = 5;

/// Everything the service needs to start.
pub struct Config {
    /// Where the HTTP API listens.
    pub listen: SocketAddr,
    /// Node JSON-RPC endpoint.
    pub rpc_url: String,
    /// Merchant receiving address, Bech32m text (ADR-081).
    pub pay_to: String,
    /// Path of the ledger file.
    pub store_path: PathBuf,
    /// Bearer token that authorises invoice creation.
    pub api_token: Zeroizing<Vec<u8>>,
    /// HMAC key for webhooks.
    pub webhook_secret: Zeroizing<Vec<u8>>,
    /// Blocks on top of a payment before it counts.
    pub confirmations: u64,
    /// Seconds between scan cycles.
    pub poll_secs: u64,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .field("rpc_url", &redact_url(&self.rpc_url))
            .field("pay_to", &self.pay_to)
            .field("store_path", &self.store_path)
            .field(
                "api_token",
                &format_args!("<{} bytes, redacted>", self.api_token.len()),
            )
            .field(
                "webhook_secret",
                &format_args!("<{} bytes, redacted>", self.webhook_secret.len()),
            )
            .field("confirmations", &self.confirmations)
            .field("poll_secs", &self.poll_secs)
            .finish()
    }
}

impl Config {
    /// Reads the configuration through `get` (the environment in production,
    /// a map in tests).
    ///
    /// # Errors
    ///
    /// [`CheckoutError::Config`] naming the first variable that is missing or
    /// wrong. The message never contains a secret's value.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let required = |name: &str| {
            get(name)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| CheckoutError::Config(format!("{name} is not set")))
        };
        // Secret text is moved straight into a zeroizing buffer, so the only copy
        // that outlives this function is the zeroized one.
        let required_secret = |name: &str| required(name).map(Zeroizing::new);
        let api_token = secret(
            &required_secret("MAYA_CHECKOUT_API_TOKEN")?,
            "MAYA_CHECKOUT_API_TOKEN",
        )?;
        let webhook_secret = secret(
            &required_secret("MAYA_CHECKOUT_WEBHOOK_SECRET")?,
            "MAYA_CHECKOUT_WEBHOOK_SECRET",
        )?;
        if api_token.as_slice() == webhook_secret.as_slice() {
            return Err(CheckoutError::Config(
                "the API token and the webhook secret must differ".into(),
            ));
        }
        let listen = get("MAYA_CHECKOUT_LISTEN").unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
        let listen = listen
            .parse()
            .map_err(|_| CheckoutError::Config("MAYA_CHECKOUT_LISTEN is not host:port".into()))?;
        let confirmations = number(&get, "MAYA_CHECKOUT_CONFIRMATIONS", DEFAULT_CONFIRMATIONS)?;
        if confirmations == 0 {
            return Err(CheckoutError::Config(
                "MAYA_CHECKOUT_CONFIRMATIONS must be at least 1".into(),
            ));
        }
        // A depth beyond the retained block ids could never be checked for reorgs.
        if confirmations > RETAIN_BLOCKS {
            return Err(CheckoutError::Config(format!(
                "MAYA_CHECKOUT_CONFIRMATIONS must be at most {RETAIN_BLOCKS}"
            )));
        }
        // A relative default would open an empty ledger from whatever directory the
        // service was started in, and forget every pending invoice.
        let store_path = PathBuf::from(required("MAYA_CHECKOUT_STORE")?);
        if !store_path.is_absolute() {
            return Err(CheckoutError::Config(
                "MAYA_CHECKOUT_STORE must be an absolute path".into(),
            ));
        }
        Ok(Self {
            listen,
            rpc_url: required("MAYA_CHECKOUT_RPC")?,
            pay_to: required("MAYA_CHECKOUT_PAY_TO")?,
            store_path,
            api_token,
            webhook_secret,
            confirmations,
            poll_secs: number(&get, "MAYA_CHECKOUT_POLL_SECS", DEFAULT_POLL_SECS)?,
        })
    }
}

/// A secret of at least [`MIN_SECRET_BYTES`], held zeroized.
fn secret(value: &str, name: &str) -> Result<Zeroizing<Vec<u8>>> {
    if value.len() < MIN_SECRET_BYTES {
        return Err(CheckoutError::Config(format!(
            "{name} must be at least {MIN_SECRET_BYTES} bytes"
        )));
    }
    Ok(Zeroizing::new(value.as_bytes().to_vec()))
}

/// The URL with any `user:password@` removed, for logs. Only the authority is
/// searched, so an `@` in the path is left alone.
#[must_use]
pub fn redact_url(raw: &str) -> String {
    let Some(start) = raw.find("://").map(|i| i + 3) else {
        return raw.to_owned();
    };
    let authority_end = raw[start..].find('/').map_or(raw.len(), |i| start + i);
    match raw[start..authority_end].rfind('@') {
        Some(at) => format!("{}<redacted>{}", &raw[..start], &raw[start + at..]),
        None => raw.to_owned(),
    }
}

fn number(get: &impl Fn(&str) -> Option<String>, name: &str, default: u64) -> Result<u64> {
    match get(name) {
        None => Ok(default),
        Some(text) => text
            .parse()
            .map_err(|_| CheckoutError::Config(format!("{name} is not a whole number"))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::collections::HashMap;

    const TOKEN: &str = "token-token-token-token-token-token-aa";
    const SECRET: &str = "secret-secret-secret-secret-secret-bb";

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    const STORE: &str = if cfg!(windows) {
        r"D:\checkout\ledger.json"
    } else {
        "/var/lib/maya/ledger.json"
    };

    fn base() -> Vec<(&'static str, &'static str)> {
        vec![
            ("MAYA_CHECKOUT_RPC", "http://127.0.0.1:9944"),
            ("MAYA_CHECKOUT_PAY_TO", "tmaya1example"),
            ("MAYA_CHECKOUT_API_TOKEN", TOKEN),
            ("MAYA_CHECKOUT_WEBHOOK_SECRET", SECRET),
            ("MAYA_CHECKOUT_STORE", STORE),
        ]
    }

    #[test]
    fn the_ledger_path_must_be_set_and_absolute() {
        // A relative default would open an empty ledger from whatever directory
        // the service was started in, and forget every pending invoice.
        let without = base()
            .into_iter()
            .filter(|(k, _)| *k != "MAYA_CHECKOUT_STORE")
            .collect::<Vec<_>>();
        assert!(Config::from_lookup(lookup(&without)).is_err());
        let mut relative = base();
        relative[4] = ("MAYA_CHECKOUT_STORE", "ledger.json");
        assert!(Config::from_lookup(lookup(&relative)).is_err());
    }

    #[test]
    fn debug_never_prints_rpc_credentials() {
        let mut pairs = base();
        pairs[0] = (
            "MAYA_CHECKOUT_RPC",
            "https://operator:hunter2-password@node.example:9944",
        );
        let cfg = Config::from_lookup(lookup(&pairs)).unwrap();
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("hunter2-password"));
        assert!(shown.contains("node.example:9944"));
    }

    #[test]
    fn loads_with_defaults() {
        let cfg = Config::from_lookup(lookup(&base())).unwrap();
        assert_eq!(cfg.confirmations, DEFAULT_CONFIRMATIONS);
        assert_eq!(cfg.listen.to_string(), DEFAULT_LISTEN);
    }

    #[test]
    fn short_secret_is_refused() {
        let mut pairs = base();
        pairs[3] = ("MAYA_CHECKOUT_WEBHOOK_SECRET", "too-short");
        assert!(Config::from_lookup(lookup(&pairs)).is_err());
    }

    #[test]
    fn equal_token_and_secret_are_refused() {
        let mut pairs = base();
        pairs[3] = ("MAYA_CHECKOUT_WEBHOOK_SECRET", TOKEN);
        assert!(Config::from_lookup(lookup(&pairs)).is_err());
    }

    #[test]
    fn debug_never_prints_a_secret() {
        let cfg = Config::from_lookup(lookup(&base())).unwrap();
        let shown = format!("{cfg:?}");
        assert!(!shown.contains(TOKEN));
        assert!(!shown.contains(SECRET));
        assert!(shown.contains("redacted"));
    }
}
