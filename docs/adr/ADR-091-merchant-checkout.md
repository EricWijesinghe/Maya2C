# ADR-091: Merchant checkout: payment link, amount matching, detached service

**Status:** Accepted, 2026-11-10 (non-consensus; the node is not touched)
**Date:** 2026-11-10
**Related:** [ADR-046](ADR-046-universe-scale-supply.md), [ADR-068](ADR-068-forever-mining.md), [ADR-081](ADR-081-address-text-bech32m.md), [ADR-083](ADR-083-hub-wallet-and-chat-pages.md); `apps/checkout`, `apps/wallet-gui/core/src/payment.rs`, [ecosystem catalog](../ecosystem/CATALOG.md) row `merchant-checkout`

## Context

A shop wants to take payment on Maya2C without running a node or holding a
key on its web server. The catalog row builds on the wallet's payment request
code. Three choices needed a written decision:

1. **The payment link format.** The Hub-launch work was expected to define a
   `maya2c-wallet://pay?...` route. It does not exist in this tree or on any
   remote branch (checked 2026-11-10). The only defined payment URI is the
   `maya:` form in `payment.rs`, which the wallet already parses.
2. **How a payment is tied to an invoice.** The chain carries no memo. An
   output has a recipient, an amount and an index, nothing else.
3. **Where the service lives.** It must not put the shop's signing or
   webhook path inside the node graph.

## Decision

1. **Payment link: the `maya:` form, as payment request v2 (ADR-086).**
   `maya:<bech32m address>?amount=<base units>&id=<percent-encoded order id>`.
   The recipient is Bech32m (ADR-081). The order id travels as `id`. The v1
   parser in `payment.rs` reads `amount` and `label` and ignores `id`, so an
   older wallet still pays the right address and amount. `label`, `message`
   and `exp` are not emitted: `exp`'s unit is not fixed by this ADR. No new
   scheme is invented. The `maya2c-wallet://pay` route named in the brief is
   not defined anywhere in the tree (checked 2026-11-10); when it is, it
   should carry these same fields and this ADR is amended.
2. **Exact-amount binding, with a durable claim.** (Revised 2026-11-11 after
   the PR #418 security review, finding C1: the earlier "at least the amount"
   rule let a payment settle the wrong invoice.)
   - An output settles an invoice only if it pays the invoice's recipient,
     exactly the amount due, in a block above the invoice's creation height and
     no later than its expiry height.
   - Each live invoice holds its amount: pending and detected invoices, and
     confirmed ones whose webhook is not yet delivered (a reorg can still revert
     those to pending). A new invoice is quoted the requested amount, or the
     next free base unit above it (24 decimals, ADR-068). The shop shows the
     quoted `amount`, which is the key a payment is matched by. Nothing is
     refused, so a stranger's open invoice at the shop's price cannot block
     checkout. A quote can move at most `MAX_AMOUNT_OFFSET` (1,000) base units
     above the request. This replaces the earlier refusal (409), which let one
     unpaid invoice block every other customer at that price, and it closes the
     reorg case where a reverted invoice could share an amount with a newer one.
   - If more than one pending invoice fits an output, nothing settles. The
     output is recorded as ambiguous.
   - Every other payment to the merchant (over, under, late, early, or with no
     invoice) is recorded as an `unmatched` payment, readable by the merchant
     at `GET /v1/unmatched`. It never settles. Refunds and manual assignment
     are the shop's decision.
   - Each output (`txid:index`) is claimed once, whatever it matched. The claim
     is written in the same commit as the scan position, so a restart cannot
     forget it.
6. **Settlement waits for depth, and reorgs roll back.** (Finding H1.)
   - The scan records each block's id. A block whose parent id differs from
     the stored id at the height below it means that stored block was reorged
     out. The service rolls back over it: detected payments and unmatched
     records from that height go back to pending, and the scan resumes from
     below it.
   - A payment is confirmed only when the chain has `confirmations - 1` blocks
     on top of its block, and its block id still matches the stored id at that
     height. Otherwise it is reverted.
   - A payment whose webhook was already delivered is not reverted. It is
     logged as an error for the operator to resolve.
   - Each scan first compares the stored block at the scan position with the
     node's block at that height. Ids chain, so a match at the top covers every
     block below it. A same-height competing branch (tip unchanged, earlier
     blocks replaced) is caught this way, and the scan rolls back to the fork
     point before anything is confirmed. Without this check, a tip that did not
     move would never be compared.
   - Block ids live in a sidecar file (`<ledger>.blocks`), rewritten when they
     change. The ledger file is not, so an ordinary block does not rewrite the
     invoices and claims.
   - A webhook result is recorded only if the invoice is still confirmed with
     the same payment. A reverted invoice is never marked delivered.
   - A reorg that removes an already-delivered payment flags the invoice
     `reorged` and logs an error. `GET /v1/alerts` (bearer token) lists flagged
     invoices. The webhook cannot be recalled, so the merchant resolves it.
   - Reorg checks cover the last 256 blocks (`store::RETAIN_BLOCKS`). When the
     stored parent id is missing, the batch's check is skipped and logged. Upgrade
     path: anchor to the BFT checkpoint (`get_checkpoint`).
   - A residual window remains: a reorg between a scan and the next webhook
     send can still send a webhook for a payment that was reorged after that
     scan. It is bounded by the poll interval, and the next scan reverts it.
     Upgrade path: send only after a scan that verified the frontier.
3. **A detached service: `apps/checkout`, a workspace member.** It reads the
   chain only through public JSON-RPC (`get_chain_info`,
   `get_blocks_by_height_range`) and links the address codec
   (`crates/address`, no node dependency), never `crates/node`. It is a member
   so it shares the workspace's pinned dependencies (ADR-005) and its rules are
   built and tested with the rest. The node binary does not link it, so the
   freeze does not apply.
4. **Webhooks: HMAC-SHA256 over `timestamp.nonce.body`.** Headers
   `maya-timestamp`, `maya-nonce`, `maya-signature: sha256=<hex>`. Receivers
   accept a timestamp within 300 seconds and refuse a nonce seen inside that
   window (`webhook::NonceCache`). A nonce must be exactly 32 hex characters,
   which keeps the signed string unambiguous. Delivery is retried with doubling
   delay (30 s base, 1 h cap, up to a quarter more as jitter, 8 attempts). A
   shop can ask for one failed webhook to be resent (`POST
   /v1/invoices/{id}/resend`). Each invoice event has an `event_id`, so a
   retry that the shop has already processed can be ignored.
5. **Destinations are checked twice.** HTTPS only, the default port only, no
   credentials, public names or public IP literals only, every resolved address
   public. A refusal gives one message whether the name is unresolved or
   private. The connection is pinned to the approved addresses, does not follow
   redirects, and does not use a proxy. The check runs at creation and again at
   each delivery.

## Consequences

- A payment without the shop's exact amount is not settled. The chain does not
  say whose order it pays, so it is kept as an unmatched record for the shop
  to handle by hand. Nothing is credited automatically.
- Open invoices at one price are quoted different base units. The customer
  pays the quoted amount, which the shop must show. The shop still decides how
  many open orders it allows per customer: the service bounds the quote offset,
  not the count of invoices.
- Unmatched records are capped at `UNMATCHED_CAP` (1,000). The oldest is evicted
  and logged. Claims for unmatched outputs are released with their records.
  Invoices and their claims are not pruned yet (a pruned claim could let an old
  output settle again).
- Public reads are limited per client address (50 a second) with a global
  ceiling (500 a second). These are chosen bounds, not measured capacity. Behind
  a TLS proxy every client shares the proxy's address, so only the ceiling
  applies there until a trusted forwarded-address setting exists.
- Windows durability: the directory is not fsynced there. Whether the rename is
  durable on Windows is NOT VERIFIED on the test host.
- Reorgs deeper than the retained block window (256 blocks) are not detected.
  Shortcut: depth-limited reorg check; upgrade by anchoring to the BFT
  checkpoint (`get_checkpoint`).
- Settlement is by confirmation depth, not by checkpoint, until the checkpoint
  anchor lands.
- A split payment (two outputs that together cover the amount) is not
  summed. Upgrade path: sum per invoice, with the claim per output kept.
- Mainnet is not assumed: the service reads the network from the node and
  refuses a merchant address for another network at startup.

## Alternatives considered

- **Matching on a memo in the transaction.** Needs a consensus change to
  carry a memo. Not for this step.
- **One address per invoice.** The protocol has no subaddresses or key
  derivation for receive-only addresses that the service could hold without
  the spend key.
- **Put the service in the node binary.** Would bring the webhook and HTTP
  surface under the frozen binary. Rejected.
