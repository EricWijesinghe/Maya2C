# ADR-083: Hub Wallet and Chat pages: watch-only, keys stay in their apps

**Status:** Accepted, 2026-11-10 (owner pre-approval for pre-mainnet non-consensus work, 2026-10-30)
**Date:** 2026-11-10
**Related:** [ADR-045](ADR-045-maya2c-hub.md), [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md), [ADR-081](ADR-081-address-text-bech32m.md); `apps/hub/src-tauri/src/watch.rs`, `apps/hub/ui/wallet.js`

## Context

Eric asked for every app in one Hub (2026-10-11). ADR-045 decision 2 says
the Hub never hashes, signs or holds keys, and that Wallet and Chat are
"linked in a later step rather than duplicating their key handling". This
ADR is that step's first half. The question is how a Hub page can show a
wallet without becoming one.

## Decision

1. **Watch-only balances.** The Wallet page keeps a list of up to 20
   addresses the user pastes. These are public data, kept in the webview's
   `localStorage`, never a key, never a seed. The command `watch_balances`
   reads `get_chain_info` once and then `get_balance` for each address,
   through the Hub's `check_url`'d RPC with no redirects.
2. **Each address is checked before it is sent.** `payout_bytes` (ADR-081)
   refuses an address for another network by name. A refused or unanswered
   address becomes an error in its own row, never zero. A balance arrives
   as a decimal string and is drawn exactly (BigInt; 24 decimals, ADR-068).
3. **No signing, sending or receiving in the Hub.** The page says so and
   points to the Wallet app. The Chat card describes the Chat app; the Hub
   reads nothing from it.
4. **Opening the apps from the Hub goes through an OS link scheme.** Each
   app declares its own scheme (`maya2c-wallet://`, `maya2c-chat://`) in
   `tauri.conf.json` under `plugins.deep-link.desktop.schemes`, so the
   installer registers it. The OS starts the installed app; the Hub never
   resolves or runs a binary path. Status: **built** on `feat/hub-launch`
   (2026-11-10). Official plugins, exact pins: `tauri-plugin-deep-link`
   2.6.3, `tauri-plugin-single-instance` 2.5.5 (`deep-link` feature),
   `tauri-plugin-opener` 2.7.2 in the Hub only. The Hub's capability allows
   `opener:allow-open-url` for those two URLs and nothing else. Wallet
   routes: `open`, and `pay?to=<maya1…>&amount=<decimal>` which only
   prefills Send (no signing, no sending). Chat accepts `open` only. Every
   other link is refused by a strict parser (`apps/*/src-tauri/src/deeplink.rs`).
   **Not yet verified:** registration in an installed NSIS/MSI or `.desktop`
   build, and the live click-through from the Hub. Those need an installed
   build, recorded in `docs/app-link-schemes.md` once run.

## Consequences

- The Hub gains read access to balances it was not asked about before. The
  node already serves them to anyone; the cost is RPC load, bounded at 20
  calls per 15 s poll.
- A watched address is visible to anyone with access to this user profile.
  It is public chain data, but it links a person to an address. The page
  stores nothing else.
- Opened in a plain browser, the page shows synthetic balances under the
  existing preview banner (ADR-045 decision 4).
