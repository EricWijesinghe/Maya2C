# ADR-088: Chat pay hand-off: the chat asks, the wallet signs

**Status:** Accepted for implementation, 2026-11-10. Non-consensus: the node, its RPC and every consensus structure are unchanged. The chat crate stays detached (ADR-031). Wallet-side route and OS link registration are NOT in this change (see "Not done").
**Date:** 2026-11-10
**Related:** [ADR-031](ADR-031-maya-chat.md) (chat, detached), [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) (signed receipts), [ADR-081](ADR-081-address-text-bech32m.md) (bech32m text form), [ADR-083](ADR-083-hub-wallet-and-chat-pages.md) (decision 4: OS link scheme, dependency review first); `docs/ECOSYSTEM.md` section 3; `docs/ecosystem/CATALOG.md` row `chat-pay`.

## Context

ECOSYSTEM.md section 3 says chat asks, the wallet shows the full transaction
and signs, and chat never holds funds. Nothing connects the two yet. The
chat app has no payment message, and the wallet has no route that accepts
a request from outside.

Three parties, three keys:

- the **payee** (requests money): its chat identity, which is also its chain
  account (`apps/chat/src/identity.rs`: the identity address is the chain
  address for the same ML-DSA-65 key);
- the **payer** (pays): its chat identity for the receipt, and its wallet
  key for the transaction. The wallet key never enters the chat process;
- the **chain**, which is the only authority for "was it paid".

## Decision

### 1. Direction: the payee requests, the payer pays

A request is an offer from the payee: "pay `amount` to me". `to` is the
payee's own address. This is the only direction in which the signed `to`
field is what the payer needs to check, and the only one in which the
payee can verify the payment itself. A request for money to a **third**
address is refused by construction: the signer's address must equal `to`.
That closes "phishing request names someone else's account" without a
policy engine. Shortcut: a payee cannot request to a second account it
owns. Upgrade when a user asks for it, with a separate signature over
that account.

### 2. Three messages, all inside the session AEAD

Each is a session plaintext with its own marker, like receipts (ADR-074).
The client refuses user text that starts with any marker, so typed text
cannot pose as a request or a receipt.

| Marker | Sent by | Carries | Signed by |
|---|---|---|---|
| `\0maya-chat pay v1\0` | payee | `to`, amount, memo, expires, nonce | payee identity |
| `\0maya-chat paid v1\0` | payer | request id, tx id | payer identity |

The chat's own session already authenticates the sender, so the identity
signature is what makes a request **transferable evidence**: the signed
statement is what the payee keeps as the request record.

### 3. Request fields and limits

- `to`: 32-byte chain address. Text form (`maya1…`, `tmaya1…`) only in the
  URI and the UI, decoded by `maya-address` with the expected network. A
  wrong-network address is refused by name (ADR-081).
- `amount`: decimal base units (ADR-046, 256-bit). Digits only, no leading
  zero, nonzero, at most 78 digits, and not above the U256 maximum. Kept as a
  string on the wire so the chat crate needs no big-integer dependency.
- `memo`: at most 140 bytes of UTF-8. No C0/C1 control characters, no
  newline, no bidirectional controls (U+202A–U+202E, U+2066–U+2069) and no
  zero-width or direction marks (U+200B–U+200F). It is shown as text only
  (`textContent`), never as HTML, and it is labelled as the payee's claim.
- `expires_at`: must be in the future, and at most 24 hours from issue. The
  24-hour cap is a policy choice, not a measurement; it bounds how long a
  pay button stays live.
- `nonce`: 16 bytes from the operating system's generator. Makes two requests
  for the same amount distinct ids.

The signed digest is a BLAKE3 derive-key over every field, each
length-prefixed. The request id is that digest. It is not bound to the
session: a request is the payee's offer, the AEAD already binds the
message to its session, and replay across sessions is refused by the
payer's ledger on the request id.

### 4. Receipt: the payer's chat says it paid, the payee checks the chain

After the wallet returns a transaction id, the payer's chat posts a `paid`
message: request id, tx id, signed by the payer's identity over the payer,
the payee, the request id and the tx id.

The payee accepts it only if all of these hold:

1. the signature verifies against the session peer's key, whose address is
   the peer (`Client::open_frame`);
2. the request id is one the payee issued and has not settled (the ledger);
3. the tx id has not settled another request (the ledger);
4. **on the chain**: the canonical block containing the tx id has an output
   to the payee whose amount is at least the requested amount. The payee
   scans canonical blocks from the height it recorded at issue, up to the
   tip, through `get_block_by_height`. Scanning is bounded
   (`pay::MAX_SCAN_BLOCKS`); a request whose payment is not found inside the
   bound stays open.

The receipt is a claim. The chain check is the authority. A forged receipt
with a real tx id for another payee fails step 4, because the output is to
the wrong recipient.

Shortcut: step 4 accepts any canonical block, with no finality depth yet.
Upgrade when the chain exposes a finality check that the chat can call
(`get_bft_status` or the checkpoint method).

### 5. The URI contract: deep links carry prefill, never authority

Outbound (chat to wallet):

```text
maya2c-wallet://pay?to=<bech32m>&amount=<whole coins, decimal>&request=<64 lowercase hex>
```

Return (wallet to chat):

```text
maya2c-chat://paid?request=<64 lowercase hex>&tx=<64 lowercase hex>
```

The amount is in whole coins, as the wallet's route reads it (decimal, at
most `maya_amount::COIN_DECIMALS` places, nonzero). The memo and the expiry
are not in the link: the memo is shown on the chat card, and the expiry is
enforced by the payer's chat before it sends the receipt. The wallet's
clear-sign view shows the recipient and the amount.

Parsing is strict. Each key appears once, the key set is exact, the link is
printable ASCII and at most 512 bytes (the wallet's bound), and each value is
checked as the request itself would be.

The wallet treats the link as prefill. It shows the clear-sign confirmation
(MP22: `apps/wallet-gui/core/src/confirm.rs`) with the recipient and the
amount, and the user approves. The link cannot make the wallet sign.

The chat accepts a `paid` return link only as a prefill. The user confirms
"I paid this", and the chat then signs the receipt. A web page or another
local app that opens the scheme cannot make the chat send a receipt on its
own.

Coordination: `feat/hub-launch` (PR #411) already accepts
`maya2c-wallet://pay?to=…&amount=…`. It refuses `request`, so its route
needs that key added before this flow works end to end. The chat's own
`maya2c-chat://paid` route is not in `apps/chat-gui/src-tauri/src/deeplink.rs`
yet, which accepts only `open`.

`parse_pay_uri` and `parse_paid_uri` in `apps/chat/src/pay_link.rs`
are the contract. The wallet route must parse exactly what they accept, and
its tests use the same vectors.

### 6. Coordination with feat/hub-launch

At writing, `origin/feat/hub-launch` is not pushed, so there is no route
parser to read. This ADR fixes the scheme, the keys and the strictness. The
hub-launch branch registers the scheme in the OS (ADR-083 decision 4). The
wallet route is a follow-up on that branch: it must accept the outbound form
above and reject everything else. When that branch is pushed, reconcile
against its parser before merge.

## Threats and where each is tested

| Threat | Control | Test |
|---|---|---|
| Spoofed request: `to` swapped | `to` signed; a changed `to` fails | `pay_tests::address_swap_fails_the_signature` |
| Request names a third party | signer address must equal `to` | `pay_tests::request_to_a_third_party_is_refused` |
| Forged sender | ML-DSA check against the session peer's key | `pay_tests::request_from_another_identity_is_refused` |
| Replay of a request | ledger refuses a request id seen before | `pay_tests::a_replayed_request_is_refused` |
| Stale request | expiry checked; TTL capped at 24 h | `pay_tests::expired_and_too_long_requests_are_refused` |
| Amount tampering | amount signed; malformed amounts refused | `pay_tests::amount_tampering_fails` and `pay_tests::malformed_amounts_are_refused` |
| Phishing text in memo | length and character rules; text only | `pay_tests::memo_with_controls_or_bidi_is_refused` |
| Replayed `paid` receipt | request settled once | `pay_tests::a_replayed_paid_receipt_is_refused` |
| Tx reused for two requests | txid recorded once | `pay_tests::one_transaction_cannot_settle_two_requests` |
| Receipt for a payment to someone else | on-chain output must be to the payee | `pay_tests::payment_to_another_address_does_not_settle` |
| Deep-link injection | strict keys, duplicate and unknown keys refused, length cap | `pay_link` unit tests |
| User text posing as a request | markers refused in `queue` | `pay_tests::typed_text_cannot_pose_as_a_request` |
| Wrong-network address | `maya-address` decodes for the expected network | `pay_tests::wrong_network_address_is_refused` |

## Consequences

- The chat gains two message types and a small persisted ledger (issued
  request ids, settled tx ids, seen incoming ids). The ledger is bounded: at
  most `pay::MAX_LEDGER` entries, oldest settled entries dropped first.
- The payee's chat makes RPC reads at settlement time, not at send time.
- The chat crate gains one path dependency, `maya-address` (thiserror only,
  already in the workspace). No new external crate.
- A chat user who never pays or requests is unaffected: no new traffic.

## Not done

- The wallet route, its parser and the clear-sign view for a chat request
  (wallet-gui; on `feat/hub-launch`).
- OS registration of `maya2c-wallet` and `maya2c-chat` schemes, and the
  deep-link plugin. ADR-083 requires its dependency review first.
- Refunds, partial payments, multiple payees, group requests.
- Finality depth for the on-chain check (see decision 4).
