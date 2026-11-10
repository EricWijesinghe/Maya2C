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
2. **Matching is by recipient and amount, with a durable claim.**
   - An output settles an invoice only if it pays the invoice's recipient, at
     least the amount due, in a block above the invoice's creation height and
     no later than its expiry height.
   - The service refuses a new open invoice whose recipient and amount equal
     an open invoice's. The amount is then the key a payment is matched by,
     and it identifies one invoice while the window is open. The shop changes the amount by
     one base unit (24 decimals, ADR-068) if it hits this.
   - Each output (`txid:index`) settles at most one invoice. The claim is
     written to the ledger in the same commit as the scan position, so a
     restart cannot forget it.
   - Underpayment never settles. Overpayment settles and reports the amount
     received; refunds are the shop's decision.
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
   window (`webhook::NonceCache`). Delivery is retried with doubling delay
   (30 s base, 1 h cap, 8 attempts). Each invoice event has an `event_id`, so
   a retry that the shop has already processed can be ignored.
5. **Destinations are checked twice.** HTTPS only, no credentials, public
   names or public IP literals only, every resolved address public. The
   connection is pinned to the approved addresses and does not follow
   redirects. The check runs at creation and again at each delivery.

## Consequences

- A payment without the shop's amount is invisible to the service. That is
  intended: the chain does not say whose order it pays.
- Two concurrent invoices cannot share an amount, so a shop with very many
  open orders at one price needs to vary the amount.
- Reorgs below the scan position are not re-examined. The service relies on
  finality plus the confirmation depth it is configured with. Upgrade path:
  check the BFT checkpoint (`get_checkpoint`) before confirming, when shops
  need that guarantee.
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
