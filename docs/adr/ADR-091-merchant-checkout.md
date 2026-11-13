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
2. **A deposit address per invoice, exact amounts, a durable claim.**
   (Revised 2026-11-13 after the second PR #418 security review, finding C1.
   The first revision bound payments by a distinct quoted amount per invoice;
   an amount is not a binding a payer controls, so that was replaced.)
   - A transaction has no memo field (`crates/node/src/core/transaction.rs`
     `TxOutput` is recipient and amount), and the node is frozen, so the
     binding is the address. The wallet derives SLIP-0010 accounts with
     hardened indices only (`apps/wallet-gui/core/src/hd.rs`), so there is no
     watch-only key the service could derive addresses from. The merchant
     derives a pool of accounts in the wallet and gives the service the public
     addresses (`MAYA_CHECKOUT_ADDRESSES`, one Bech32m address per line). The
     service holds no key.
   - Each invoice is given a pool address no invoice has ever had. An address is
     never given out again, not even after its invoice expires, so a late
     payment still names its invoice. An exhausted pool refuses new invoices
     (409) rather than reuse an address.
   - An output is only ever considered for the invoice that owns its address.
     It settles that invoice only if the invoice is pending, the amount is
     exact, and the block is above the creation height and no later than the
     expiry height. The invoice is quoted exactly the amount the shop asked.
   - Every other payment to a merchant address is recorded as `unmatched`,
     with its invoice id and a reason (`underpaid`, `overpaid`, `early`, `late`,
     `already_settled`, or `unassigned` for a pool address with no invoice),
     readable at `GET /v1/unmatched`. It never settles and is never applied to
     another invoice. Refunds are the shop's decision.
   - Each output (`txid:index`) is claimed once, whatever it matched. The claim
     is written in the same commit as the scan position, so a restart cannot
     forget it.
6. **Settlement waits for DAG-BFT finality, and reorgs roll back.**
   (Finding H1, revised 2026-11-13: confirmation depth replaced by finality.)
   - The scan records each block's id. A block whose parent id differs from
     the stored id at the height below it means that stored block was reorged
     out. The service rolls back over it: detected payments and unmatched
     records from that height go back to pending, and the scan resumes from
     below it.
   - A payment is confirmed, and its webhook queued, only when its block is at
     or below the node's newest checkpoint (`get_checkpoint`, ADR-038, the call
     `apps/sentinel` reads finality with) and the checkpoint's block id equals
     the id this service scanned at that height, so the checkpoint and the
     payment are on one branch. The payment's own block id must also still
     match. No checkpoint, a checkpoint above the scanned blocks or below the
     retained ids, or a failed read: nothing confirms. The checkpoint is read
     before the tip each cycle. Its signatures are not checked: the service
     already trusts its node for every block it reads.
   - A payment seen but not yet final that a reorg removes reverts the invoice
     to pending, and its webhook is never sent.
   - A payment whose webhook was already delivered is not reverted. It is
     logged as an error for the operator to resolve. With confirmation at
     finality this needs a finalized block to be replaced.
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
   - Block ids are kept for the last 256 blocks (`store::RETAIN_BLOCKS`). When
     the stored parent id is missing, the batch's check is skipped and logged.
     Because only finalized payments are announced, the earlier window (a reorg
     between a scan and the webhook send) no longer reaches a webhook.
3. **A detached service: `apps/checkout`, a workspace member.** It reads the
   chain only through public JSON-RPC (`get_chain_info`,
   `get_blocks_by_height_range`, `get_checkpoint`) and links the address codec
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

- A payment without the exact amount is not settled. It is kept as an
  unmatched record naming its invoice, for the shop to refund by hand.
  Nothing is credited automatically.
- The merchant must keep the address pool topped up: one address per invoice,
  never reused. Each pool address is a wallet account the merchant can spend.
- Confirmation takes as long as finality does; a node without DAG-BFT
  checkpoints confirms nothing.
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
- A checkpoint more than 256 blocks below the scan position cannot be matched
  to a stored id, so confirmation waits (fails closed) until finality catches up.
- Expiry is decided at scan height, not at finality: an invoice expired and then
  reorged so its payment lands inside the window records that payment as
  `already_settled` for a refund, never as a settlement.
- A split payment (two outputs that together cover the amount) is not
  summed. Upgrade path: sum per invoice, with the claim per output kept.
- Mainnet is not assumed: the service reads the network from the node and
  refuses a pool address for another network at startup.

## Alternatives considered

- **Matching on a memo in the transaction.** Needs a consensus change to
  carry a memo. Not for this step.
- **Deriving each invoice's address in the service.** The wallet's SLIP-0010
  derivation is hardened only, so the service would need the seed, which puts
  a spend key on a web-facing host. Rejected: the merchant loads a pool of
  public addresses instead.
- **Binding by a distinct quoted amount per invoice** (this ADR's first
  revision). Any payer who sends another invoice's amount pays that invoice,
  and quotes drift above the shop's price. Replaced 2026-11-13.
- **Put the service in the node binary.** Would bring the webhook and HTTP
  surface under the frozen binary. Rejected.
