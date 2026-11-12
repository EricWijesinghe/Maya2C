# sdks/rust — Maya2C for Rust

`maya2c-sdk`: a typed, blocking client for the Maya2C API gateway (REST
`/v1`). It links no consensus crate, holds no keys and signs nothing.

## Install

### Not published yet, build from source

`maya2c-sdk` is not on crates.io (`crates.io/api/v1/crates/maya2c-sdk`
answered 404 on 2026-11-13; the crate is `publish = false`). Depend on the
repository instead. Edition 2024, Rust 1.88 or later.

```bash
cargo add maya2c-sdk --git https://github.com/EricWijesinghe/Maya2C
```

or in `Cargo.toml`:

```toml
[dependencies]
maya2c-sdk = { git = "https://github.com/EricWijesinghe/Maya2C" }
```

Pin a `rev = "<commit>"` for a build that does not move with master.

## Send a transfer

The gateway does not sign, and neither does this crate: the transfer is signed
by `l1-wallet`, the Rust wallet shipped with the node, and handed here as hex.
Against `cargo xtask kit up` (gateway on `127.0.0.1:8080`), from the
repository root:

```bash
export L1_WALLET_PASSWORD=devnet-only-password   # the devnet constant in xtask/src/devnet.rs
GENESIS=$(curl -s http://127.0.0.1:8080/v1/chain | sed -n 's/.*"genesis":"\([0-9a-f]*\)".*/\1/p')
target/debug/l1-wallet --keystore target/up/wallet.key --rpc-url http://127.0.0.1:32000 \
  send --yes --genesis "$GENESIS" --no-broadcast \
  --to 7777777777777777777777777777777777777777777777777777777777777777 --amount 500
# prints `raw: <hex>` and `txid: <hex>`; put the raw hex in RAW_TX
```

```rust
use std::time::Duration;

fn main() -> Result<(), maya2c_sdk::Error> {
    let client = maya2c_sdk::Client::new("http://127.0.0.1:8080")?;
    let fees = client.fee_info()?;
    println!("base fee {} per byte, paid to {}", fees.base_fee, fees.collector);

    let raw = std::env::var("RAW_TX").unwrap_or_default();
    let hash = client.send_raw_transaction(&raw)?; // accepted into the mempool
    let height = client.wait_for_transaction(&hash, Duration::from_secs(60))?;
    println!("{hash} is in block {height}");

    let recipient = "7777777777777777777777777777777777777777777777777777777777777777";
    let balance = client.balance(recipient)?; // 64 hex characters or Bech32m
    println!("{} units, next nonce {}", balance.balance, balance.nonce); // U256 (ADR-046)
    Ok(())
}
```

Other calls: `health`, `chain_info` (genesis id, chain id, height), `supply`,
`block(height)`, `send_sealed_transaction`. Every error is a typed
`maya2c_sdk::Error`; an address is checked before any request is sent. GET
requests are retried on 429 and 503 within a cap; a POST is never retried.

## What is checked

- `tests/client_tests.rs` runs the client against a mock gateway (retries, the
  256-bit balance, address checks) in CI's `cargo nextest run --workspace`.
- The `no_run` example in `src/lib.rs` compiles under
  `cargo test -p maya2c-sdk --doc`; nextest skips doctests, so CI does not.
- `cargo xtask sdk-e2e` does not cover this SDK yet (`--lang` is `ts`,
  `python` or `go`), so the transfer above is not run by CI.
