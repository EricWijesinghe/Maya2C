# ADR-116: Browser-extension wallet — signing in sdk-wasm, `window.maya2c` per-origin provider

Status: Accepted (wallet tooling only; no consensus, wire or state change)
Date: 2026-11-12

## Context

`docs/ecosystem/CATALOG.md` row `wallet-extension` (job P-9) gates the
browser wallet on "an ADR for the dApp connection protocol". Four questions
had to be settled.

1. **Where signing lives.** The brief said `sdks/sdk-wasm` already signs.
   It did not: it exported address derivation and verification only. Signing
   in TypeScript would be a second, hand-maintained copy of a post-quantum
   scheme (`sdks/sdk-js` refuses that for the same reason).
2. **How a browser derives the desktop wallet's accounts.** The desktop path
   (`apps/wallet-gui/core/src/hd.rs`) goes through `custom_l1_node`, which
   cannot target wasm (RocksDB).
3. **How keys are stored.** WebCrypto offers PBKDF2 but not Argon2id.
4. **What a web page may ask for, and how the wallet knows who asked.**

## Decision

- **Signing and derivation are Rust in `sdks/sdk-wasm/src/wallet.rs`**: BIP-39
  seed, SLIP-0010 `m/44'/2992'/account'/0'/index'`, the node's chain-key split
  into ML-DSA-65 and SLH-DSA-SHA2-128s seeds, deterministic hybrid signing,
  and the version-5 plain-transfer frame. It copies the node's construction
  rather than inventing one, and `sdks/sdk-wasm/tests/desktop_parity_tests.rs`
  pins it: seed, public key and address equal `maya-wallet-core`'s; a signed
  transfer equals the node's `Transaction::to_bytes` byte for byte and passes
  `Transaction::verify` (and fails on another chain tag).
  `spec/wallet-derivation-vectors.json` carries the addresses to the
  JavaScript tests, so the desktop, the Rust copy and the shipped `.wasm`
  are checked against one file.
- **Vault:** the mnemonic is encrypted with AES-256-GCM (WebCrypto,
  non-extractable key) under a key from **Argon2id run in wasm** at the
  desktop vault's parameters (64 MiB, 3 passes, 1 lane). The unlocked seed
  lives only in the background context's memory; when the browser stops the
  service worker, the wallet locks. Nothing secret is put in
  `storage.session`.
- **Provider:** `window.maya2c.request({ method, params })`, after EIP-1193's
  shape (a promise, errors with numeric codes 4001/4100/4200/-32602) so dapp
  code ports, with Maya2C's own methods: `connect`, `accounts`, `chain`,
  `sendTransfer`, `disconnect`.
  - **Per-origin grant.** A site sees no address until the person approves
    its exact origin. Only `https:` origins, and `http:` on loopback, can be
    granted. Only top-level frames are served, so an iframe cannot borrow
    its parent's grant.
  - **Who asked comes from the browser.** The background takes the origin
    from the runtime `sender` record of the content script, never from the
    message. Popup-only commands (unlock, approve) are accepted only from
    this extension's own pages.
  - **`sendTransfer` names its chain.** Its `genesis` parameter is required;
    a request for one chain while the wallet is connected to another is
    refused (ADR-036). A Bech32m recipient (ADR-081) written for another
    network is refused: a `tmaya1…` invoice is never paid from mainnet, and a
    `maya1…` address is never paid with test coins (the #446 lesson). The
    network is classified by genesis id, never by the node's name, as on the
    desktop (`networks.rs`); an unpinned genesis calling itself mainnet is
    refused outright.
  - **Bounded prompts.** One waiting request per origin, five in total, each
    expiring after two minutes; after the person rejects a site's prompt or
    lets it expire, that site waits 30 s before the next, doubling to at most
    10 minutes; one approval window, focused rather than
    re-opened; closing it, locking, or disconnecting rejects what waits. The
    approve button arms 0.8 s after the screen appears and only with focus.
    `chain` needs a grant, and errors other than refusals reach the page as
    a generic failure, so a site cannot probe the balance or the gateway.
  - **Clear signing, bound to the signing hash (as #271 on the desktop).**
    The background builds one frozen plan (recipient, amount, fee, fee
    collector, nonce, genesis). `sdk-wasm::wallet::confirm_transfer` renders
    it with `maya-clear-sign` — `render`, `grouped` Bech32m, `review` and
    `warning_text`, the desktop `confirm.rs` renderer, moved into that crate
    so both show the same words — and returns the BLAKE3 hash of the signing
    bytes. The popup shows only that rendering and the hash.
    `sign_transfer` takes the hash back, recomputes it, and signs nothing on
    a mismatch; a fee to anyone but the fee collector is refused in Rust. The
    address prefix is chosen by the pinned genesis lists, copied from the
    desktop and pinned to it by a test. The chain, the nonce and the balance
    are re-read before signing; if any changed, nothing is signed.
- **Lock:** 15 minutes without use of the popup (page calls do not restart
  the timer), or the browser stopping the worker. The unlocked seed is a
  `WalletSeed` held in wasm memory (`Zeroizing`); locking frees and wipes it.
  Passwords have at least 12 characters; the vault's format and salt are
  AES-GCM associated data.

## Consequences

- `sdk-wasm` gains `bip39`, `pbkdf2`, `hmac`, `sha2`, `zeroize`, `argon2`
  (all already in the workspace at the same versions the desktop wallet
  uses), plus the workspace crates `maya-clear-sign` and `maya-amount`;
  `wasm-bindgen` is pinned exactly (`=0.2.108`) because the build script
  needs a matching `wasm-bindgen-cli`. The node does not link `sdk-wasm` (dev-dependency only), so the
  frozen node binary is unchanged.
- The derivation is now in three places (node, desktop via the node, and
  `sdk-wasm`). The parity tests make a drift a failing test, not a silent
  empty account. Extracting `hybrid` into a wasm-clean crate remains the
  structural fix (see `sdks/sdk-js/README.md`).
- JS cannot guarantee zeroization: buffers are overwritten with `fill(0)`,
  strings (the mnemonic during import/unlock) cannot be. The seed itself never
  leaves wasm (`WalletSeed`), but wasm-bindgen's copies of the phrase and the
  returned vault key in linear memory are freed, not wiped. Stated in the README.
- Only account 0 and plain transfers are supported. No events
  (`accountsChanged`), no message signing, no typed payloads. Each is a later
  change to this ADR, not a quiet addition.
- Store publication is out of scope and needs Eric's `APPROVED:`.
