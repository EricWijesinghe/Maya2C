# Maya2C Wallet — browser extension

A Manifest V3 wallet for Chrome (111+) and Firefox (128+): create or import a
wallet, see the balance, send with a confirm screen, and let web apps connect
through `window.maya2c`. Design and protocol: [ADR-116](../../docs/adr/ADR-116-wallet-extension-dapp-protocol.md).

**Not published to any store.** Publishing needs Eric's `APPROVED:`. Load it
unpacked (below). It is unaudited; use test coins.

## Same words, same address as the desktop wallet

Signing and derivation run in `sdks/sdk-wasm` (Rust compiled to wasm), not in
JavaScript: BIP-39 seed, `m/44'/2992'/0'/0'/0'`, ML-DSA-65 + SLH-DSA-SHA2-128s.
`sdks/sdk-wasm/tests/desktop_parity_tests.rs` checks every step against the
desktop wallet and the node, and `spec/wallet-derivation-vectors.json` carries
the addresses to this extension's tests. Only account 0 is used.

## Build and load

Needs Rust with the `wasm32-unknown-unknown` target, `wasm-bindgen-cli` at the
exact version of the `wasm-bindgen` crate in `Cargo.lock` (the script checks),
and Node 20+.

```bash
cd apps/wallet-extension
npm install
npm run build:wasm   # cargo build sdk-wasm for wasm32, then wasm-bindgen --target web
npm run build        # dist/chrome and dist/firefox
npm run typecheck    # tsc, strict
npm test             # vitest; uses the real wasm build
```

- **Chrome / Edge / Brave:** `chrome://extensions` → Developer mode → *Load
  unpacked* → `dist/chrome`.
- **Firefox:** `about:debugging#/runtime/this-firefox` → *Load Temporary
  Add-on* → `dist/firefox/manifest.json`.

The default gateway is `http://127.0.0.1:8080` (`maya2c-gateway`'s default).
Set another under Settings; for an `https://` gateway the browser asks you to
allow access to that one site.

## For dapp developers

```js
const [account] = await window.maya2c.request({ method: "connect" });   // asks the person
const { genesis, network } = await window.maya2c.request({ method: "chain" });
const { hash } = await window.maya2c.request({
  method: "sendTransfer",
  params: { to: "tmaya1…", amount: "1000000000000000000000000", genesis }, // base units (10^24 = 1 MAYA)
});
```

| Method | Needs connect | Result |
|---|---|---|
| `connect` | — | `[hexAccountId]` after the person approves this origin |
| `accounts` | no | `[hexAccountId]`, or `[]` if not connected or locked |
| `chain` | yes | `{ genesis, network: "mainnet" \| "test" \| "impostor", name }` |
| `sendTransfer` | yes | `{ hash }` after the person approves the confirm screen |
| `disconnect` | no | `null`; withdraws this origin's grant |

Errors carry `code`: 4001 rejected by the person, expired (2 minutes), or a
request already waiting for this site; 4100 not connected / locked / origin
refused; 4200 unknown method; -32602 bad params (including a `genesis` that
is not the connected chain's, or an address of another network); -32603
anything else, with a generic message — a site learns nothing about the
balance or the gateway from an error.

One request per site waits at a time, and at most five in total. Closing
the approval window, locking, or disconnecting the site rejects what is
waiting.

## Security properties, and their limits

- **Keys at rest:** mnemonic encrypted with AES-256-GCM; key from Argon2id
  (64 MiB, 3 passes, 1 lane — the desktop vault's parameters), run in wasm.
- **Unlocked seed:** in the background's memory only. The browser stopping
  the service worker, or 15 minutes without use, locks the wallet.
- **Password:** at least 12 characters. The vault's format label and salt
  are bound to the ciphertext as AES-GCM associated data. A forgotten
  password: "Erase this wallet" (type ERASE), then import the written words.
- **Origins:** only `https:` pages, and `http:` on localhost/127.0.0.1, can
  connect; only top-level frames. The origin is the browser's `sender`
  record, never a value the page sends.
- **Network check:** network decided by genesis id; a `maya1…` address is
  refused on a test chain and a `tmaya1…`/`dmaya1…` one on mainnet; a dapp
  request for a different genesis is refused; an unpinned genesis calling
  itself mainnet is refused. The fee collector must be the chain's, and fees
  above 1 MAYA are refused.
- **What you approve is what is signed:** the confirm screen's words, the
  full grouped Bech32m recipient and sender, and the warnings are rendered in
  wasm by `maya-clear-sign` — the renderer the desktop wallet uses — from the
  transfer that will be signed, with its signing hash (BLAKE3 of the signing
  bytes) shown. `signTransfer` recomputes that hash and signs nothing if it
  differs, as the desktop's `Wallet::sign` does (#271);
  `desktop_parity_tests.rs` checks the hash and every shared field against
  the desktop's `confirm.rs` on the same transaction. Approving also
  re-checks the chain, the nonce and the balance. The approve button arms
  0.8 s after the screen appears, and only while the window has focus.
- **Address prefix:** chosen by the genesis id alone, as on the desktop, so
  an unpinned chain's addresses are shown as `dmaya1…`.
- **CSP:** `script-src 'self' 'wasm-unsafe-eval'`, no remote code, `default-src 'none'`.
- **Permissions:** `storage`, loopback host access, and per-gateway `https`
  access asked for on save.
- **Seed in wasm:** the unlocked seed is a `WalletSeed` object in wasm memory
  (`Zeroizing`), never a JS buffer; locking calls `free()`, which wipes it.
  Derivation, the confirm screen and signing are methods on it.
- **Limits:** JavaScript cannot guarantee zeroization. Buffers are overwritten
  with `fill(0)`; strings (the mnemonic while it is typed and while the vault
  opens) cannot be, and wasm-bindgen frees, not wipes, the copies of the
  phrase and of the vault key it passes across the boundary.
- **Rate limits:** one waiting prompt per site, five in total, two minutes
  each; after the person rejects a site's prompt or lets it expire, that site
  cannot prompt again for 30 s, doubling on each repeat up to 10 minutes.
  Only the popup restarts the 15-minute inactivity lock, so a page calling in
  a loop cannot keep the wallet unlocked. A compromised browser or another malicious extension with broad
  permissions is outside what this can defend against. Not externally
  reviewed.
- **BIP-39 passphrase:** used as typed, without NFKD normalization — the
  same as the desktop wallet today, so both derive the same accounts. A
  non-ASCII passphrase may not recover in other BIP-39 tools; fixing it is
  a change to both wallets together.
- **Firefox:** Firefox may treat content-script match patterns as host
  permissions the person must grant. If `window.maya2c` is missing on a
  site, allow the extension on that site from the toolbar menu.
  (Not yet checked in a real Firefox.)
- **Not pinned yet:** the mainnet genesis list is empty until the genesis
  ceremony, as on the desktop. Until then every chain is shown as "not
  pinned", and the confirm screen asks you to compare the fingerprint.

## Not built yet

Several accounts, `accountsChanged`/`chainChanged` events, message signing,
typed transactions (swaps, staking), a hardware-wallet path, and store
packaging.
