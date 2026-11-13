# ADR-138: Chat mini-apps ask; the user and the wallet decide

**Status:** Proposed, 2026-11-12. Non-consensus: no node, RPC, wire or state change. The chat crate stays detached (ADR-031). Number claimed in `adr-claims.txt`; the coordinator may renumber.
**Date:** 2026-11-12
**Related:** [ADR-031](ADR-031-maya-chat.md) (chat, detached), [ADR-088](ADR-088-chat-pay-handoff.md) (chat asks, wallet signs), [ADR-086](ADR-086-wallet-v2-seedless-recovery-and-clear-signing.md) (clear signing), `docs/chat-mini-apps.md`, `docs/ecosystem/CATALOG.md` row `chat-mini-apps` (P-45).

## Context

P-45 asks for small apps inside a chat (split a bill, a poll, a shop) and
for wallet actions in the conversation. A chat app that runs third-party
code next to an identity key and a pay button is a phishing surface: the app
can draw a fake confirm dialog, read the conversation, or call home. The
decisions below keep an app's reach to three named asks.

## Decision

### 1. A signed manifest, and nothing runs without it

A mini-app is one HTML file and a CBOR manifest: `name`, `icon_hash`,
`entry_hash` (BLAKE3), `permissions`, `publisher_key`, `signature`. The
signature is the hybrid suite `0x30` (ML-DSA-65 and SLH-DSA-SHA2-128s) over a
domain-separated BLAKE3 digest of every other field
(`apps/chat/src/miniapp.rs`). The host re-verifies the manifest and re-hashes
the entry file on every list, open and page serve. The publisher is shown as
its hybrid-suite account address.

The hybrid suite, not the chat identity's ML-DSA-65: a publisher key is
long-lived and signs code, which is where the two-family hedge earns its
11 KB.

### 2. A fixed allow-list of three permissions

`read-address`, `request-payment`, `send-message`. Any other name refuses the
whole manifest, and the allow-list is checked before the signature, so an
unknown permission is refused whoever signed it. A declared permission is
still asked of the user on first use. There is no permission to sign, to
read messages, or to reach the network.

### 3. A sandbox with no way out but postMessage

The entry file is served on the `miniapp:` scheme with its own CSP
(`default-src 'none'`, inline script and style only, `connect-src 'none'`,
no frames, forms or base) and loaded into `<iframe sandbox="allow-scripts">`:
an opaque origin with no Tauri API. The window accepts `postMessage` only
from that frame's `contentWindow` and forwards each call to the Rust bridge
with a random 16-byte token for the frame; the bridge drops any other token
(`apps/chat/src/miniapp_bridge.rs`).

### 4. Payments are confirmed every time, and only the wallet signs

`request-payment` lets an app propose `{to, amount, memo}`. The bridge checks
the fields, builds the confirm text (full Bech32m payee, amount in coins,
memo, app name and publisher), and returns a wallet link only after the user
says yes. The wallet then shows the payment again and signs. The link uses
ADR-088's `maya2c-wallet://pay?` contract (including `chain`, the chain set
for payments; an app cannot open before it is set), with a bridge intent id in place
of a signed request id.

### 5. App messages are labelled and cannot pose as payments

`send-message` text is prefixed `[<app name>]`, may not contain NUL, and may
not start with a payment marker (`pay::is_marked`). App names may not contain
brackets or direction characters.

### 6. In-chat wallet bubbles reuse ADR-088

Request, pay and receipt bubbles in `apps/chat-gui` use `pay::Request` and
`pay::Paid` unchanged. A received request's Pay button shows the same
confirm text, then the wallet link.

## Not done (shortcuts, recorded)

- Confirm text is this module's own format; PR #446's Bech32m clear-sign
  renderer replaces it once merged (`shortcut:` in `miniapp_bridge.rs`).
- The wallet link is shown for the user to copy; opening it needs
  `tauri-plugin-opener` (`shortcut:` in `wallet.js`).
- The chat network is testnet only (`shortcut:` in `pay_cmds.rs`).
- "Says it paid" bubbles are not checked against the chain in the GUI.
- No installer: apps are folders under `<app data>/miniapps/<id>/`.
- Publisher keys are not pinned or listed; any valid signature runs, with the
  publisher address shown in every prompt.

## Consequences

An app that misbehaves can at worst ask, and every ask names it. The user's
yes is the only path to the wallet, and the wallet's own confirm is the only
path to a signature.
