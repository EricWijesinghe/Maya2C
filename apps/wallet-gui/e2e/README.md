# WebDriver end-to-end suite

**These tests have never been run.**

`tauri-driver` and `msedgedriver` are both absent from the development host, and
Tauri's WebDriver support needs one of them plus a built application binary.
The specs below are written and are believed correct; nothing has executed them.

That is the same status as the Kotlin and Swift SDK bindings, and it is stated
here for the same reason: a suite that has never run is not evidence, and
listing it as passing coverage would be a green tick over something nobody
executed.

## Where the real coverage is

In `apps/wallet-gui/core`, which is where the logic lives:

| Area | Tests | Run |
|---|---|---|
| BIP-39 + SLIP-0010 derivation | `tests/hd_tests.rs` | yes |
| Vault sealing and the OS keychain | `tests/vault_tests.rs` | yes |
| Payment parsing, QR decode, signing | `tests/payment_tests.rs` | yes |
| Air-gapped framing | `src/airgap.rs`, `tests/airgap_tests.rs` | yes |
| Swap and shielded composers | `src/compose.rs` | yes |

The Tauri commands in `src-tauri/src/commands.rs` are thin wrappers over those
functions — they marshal arguments and map errors to strings. WebDriver would
exercise the marshalling and the UI; it would not exercise anything that decides
what a transaction contains.

## Running these when a driver exists

```bash
cargo install tauri-driver --locked
# Windows: msedgedriver must match the installed Edge WebView2 version
cargo build --manifest-path apps/wallet-gui/src-tauri/Cargo.toml --release

tauri-driver &
npx mocha apps/wallet-gui/e2e/*.spec.mjs
```

## What each spec covers

- `wallet_creation.spec.mjs` — create, record the phrase, unlock, derive.
- `transfer.spec.mjs` — compose, sign, submit to a mock RPC, see the balance move.
- `airgap.spec.mjs` — offline sign, step the frame animation, reassemble.

## One thing WebDriver cannot check here

The air-gapped flow's real failure mode is a **camera** missing a frame off a
**screen**. WebDriver drives the DOM; it can confirm the animation advances and
that the assembler reports the right missing frames, but it cannot tell you
whether a phone can actually read a version-20 QR code from a laptop display.
That needs a person and a phone, and `docs/wallet.md` says so.
