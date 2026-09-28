# Wallet end-to-end suite

The Maya Wallet driven through its real UI: the release build of the Tauri
application, WebView2 through `tauri-driver` and `msedgedriver`, and a small
W3C WebDriver client in Rust (`src/lib.rs`). No Node toolchain.

`tests/wallet_flows.rs` covers:
- create a wallet; the 24-word backup, with Continue disabled until the
  acknowledgement is ticked;
- the wallet view: account 0, then Add account;
- Lock; a wrong passphrase refused with its message; unlock;
- relaunch: the stored wallet opens at Unlock and the phrase is never shown
  again.

## Running it

```text
cargo tauri build --no-bundle          # in apps/wallet-gui/src-tauri
cargo install tauri-driver --locked
# msedgedriver must match the installed Edge WebView2 version

MAYA_WALLET_EXE=<abs path>/apps/wallet-gui/src-tauri/target/release/maya-wallet-gui.exe \
TAURI_DRIVER=<path>/tauri-driver.exe EDGE_DRIVER=<path>/msedgedriver.exe \
cargo test -p maya-wallet-e2e -- --ignored --nocapture
```

Measured 2026-09-28 on the Windows workstation (WebView2 and msedgedriver
154.0.4258.37): 1 passed in 3.67 s.

The application runs with its keychain in a namespace of the run's own
(`MAYA_WALLET_KEYCHAIN_SERVICE`), deleted afterwards. The test never sees,
overwrites or leaves behind the user's wallet entry.

## What running it found

The first run could not create a wallet. The UI calls
`window.__TAURI__.core.invoke`, and Tauri 2 defines that global only with
`app.withGlobalTauri`, which the configuration lacked. So every command the
wallet makes — create, unlock, send — waited forever, in the shipped build
too. It is now set.

WebDriver also could not attach at first: wry always passes WebView2
arguments of its own, which override the `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`
that `msedgedriver` uses to open a debugging port. The application now
honours that variable when it is set; it is unset in normal use.

## Not covered

- The air-gapped signing flow and sending need a node and a funded account.
  The core library's tests cover their logic (`core/tests/`).
- Reading the QR animation with a real camera is not something a DOM driver
  can check.
