//! Measures the largest secret this machine's OS keychain accepts (review
//! fix M2), by binary search on one probe entry that is deleted afterwards.
//!
//! Run only under a throwaway namespace, never the wallet's own:
//!
//! ```text
//! MAYA_WALLET_KEYCHAIN_SERVICE=com.maya2c.wallet.capprobe \
//!   cargo run -p maya-wallet-core --example keyring_cap
//! ```

use maya_wallet_core::vault::{OsKeychain, SERVICE_ENV, SecretStore};

/// Upper end of the search: well past any documented keychain limit.
const SEARCH_MAX: usize = 65_536;
const PROBE: &str = "capacity-probe";

fn main() -> Result<(), String> {
    let service = std::env::var(SERVICE_ENV).unwrap_or_default();
    if !service.contains("probe") {
        return Err(format!(
            "set {SERVICE_ENV} to a namespace containing 'probe'; refusing to touch the wallet's own entries"
        ));
    }
    let store = OsKeychain::new();
    let fits = |n: usize| store.put(PROBE, &vec![0xA5; n]).is_ok();
    let (mut ok, mut bad) = (0usize, SEARCH_MAX + 1);
    while bad - ok > 1 {
        let mid = ok + (bad - ok) / 2;
        if fits(mid) {
            ok = mid;
        } else {
            bad = mid;
        }
    }
    store.delete(PROBE).map_err(|e| e.to_string())?;
    println!(
        "OS keychain under '{service}': largest accepted secret {ok} bytes, first refused {bad}"
    );
    Ok(())
}
