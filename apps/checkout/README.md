# Maya2C checkout

Take payment in MAYA for an order. The checkout service makes a payment
link and a QR code for each order, watches the chain, and tells your shop
with a signed webhook once the payment is confirmed.

It is a separate program. It is not part of the node, and it never holds
your wallet's keys. It only needs a list of your receiving addresses, one
per invoice.

## For shop owners

**What you run.** One process, `maya2c-checkout`, on a server you control.
It needs access to a Maya2C node's RPC address. Keep it on a private
network or behind a TLS proxy; it listens on `127.0.0.1:8787` by default.

**What your shop does.**

1. Your server asks for an invoice:

   ```
   POST /v1/invoices
   Authorization: Bearer <your API token>
   {"amount": "<base units>", "memo": "order-1042",
    "expires_in_blocks": 600, "webhook_url": "https://shop.example/maya"}
   ```

   Amounts are whole base units with 24 decimals. `1 MAYA` is
   `1000000000000000000000000`. Send the amount as a plain string.

2. The reply has `pay_uri` and `id`. Show the customer the QR code from
   `GET /v1/invoices/<id>/qr.svg`, or the link itself. Any Maya2C wallet
   that scans a `maya:` code can pay it.

3. Your webhook receives one request when the payment is final (DAG-BFT
   finality, see below):

   ```json
   {"event_id": "<id>:confirmed", "event": "invoice.confirmed",
    "invoice_id": "...", "memo": "order-1042", "amount": "...",
    "amount_paid": "...",
    "txid": "...", "index": 0, "height": 12345}
   ```

**Check every webhook before you act on it.** Each request has three headers:
`maya-timestamp`, `maya-nonce` and `maya-signature`. The signature is
`sha256=` followed by HMAC-SHA256 over the text
`<timestamp>.<nonce>.<raw body>`, using the webhook secret you set. Reject a
request whose timestamp is more than 5 minutes from your clock, whose
signature does not match, or whose nonce you have already seen in those 5
minutes. Then process the `event_id` only once: a request can arrive more
than once if the first answer was lost.

**Retries.** If your server does not answer with a 2xx, the service tries
again with a growing wait (about 1 minute, then 2, 4 and so on, up to an hour)
and gives up after 8 attempts. You can always read the true state with
`GET /v1/invoices/<id>`.

**Each invoice has its own deposit address.** A Maya2C transaction carries no
memo, so the address is what ties a payment to one order. Derive a batch of
accounts in your wallet (as many as the invoices you expect), put their
public addresses in a file, one per line, and point
`MAYA_CHECKOUT_ADDRESSES` at it. The service holds no key. Every invoice gets
an address no invoice has had before, and it is never given out again, even
after the invoice expires. When the file runs out, creation answers 409: add
more addresses and restart. Show the reply's `address` (or `pay_uri`) and
`amount`.

**Only exact payments to the invoice's address settle it.**
- A payment of exactly the invoice amount, to that invoice's address, settles
  it. Anything else does not, and nothing is ever moved to another invoice.
- A customer who pays less, more, too early, too late or twice does not pay the
  invoice. Each such payment is kept as an *unmatched* record naming its
  invoice and a reason (`underpaid`, `overpaid`, `early`, `late`,
  `already_settled`, or `unassigned` for an address no invoice has yet). Read
  them with `GET /v1/unmatched` (bearer token) and refund by hand. The list is
  paged: `GET /v1/unmatched?offset=0&limit=50` (limit at most 100), and the
  reply gives `next_offset`.

**Failed webhooks.** If every retry fails, `POST /v1/invoices/<id>/resend`
(bearer token) puts it back in the queue once. A webhook already delivered
cannot be resent.

**Finality.** A payment counts, and the webhook is sent, only once DAG-BFT has
finalized its block: the block is at or below the node's newest checkpoint
(`get_checkpoint`), and that checkpoint is on the chain the service scanned.
Until then the invoice is `detected`. If a reorg removes a detected payment,
the invoice goes back to `pending` and no webhook is sent. A node with no
checkpoints confirms nothing.

**Expiry.** An invoice is open for `expires_in_blocks` blocks. A payment that
lands after that is ignored, and the invoice becomes `expired`.

## Configuration

| Variable | Required | Meaning |
|---|---|---|
| `MAYA_CHECKOUT_RPC` | yes | Node JSON-RPC URL |
| `MAYA_CHECKOUT_ADDRESSES` | yes | File of deposit addresses, one Bech32m address per line (`#` comments allowed), all on the node's network |
| `MAYA_CHECKOUT_API_TOKEN` | yes | Bearer token for creating invoices, at least 32 bytes |
| `MAYA_CHECKOUT_WEBHOOK_SECRET` | yes | HMAC key for webhooks, at least 32 bytes, different from the token |
| `MAYA_CHECKOUT_STORE` | yes | Absolute path of the ledger file. Back it up: it holds the record of which payments were used |
| `MAYA_CHECKOUT_LISTEN` | no | Default `127.0.0.1:8787` |
| `MAYA_CHECKOUT_POLL_SECS` | no | Pause between scans, default 5 |

Secrets are read from the environment, held in memory that is wiped when it
is freed, and never printed or written to the log.

## Threat model

What an attacker can try, and what stops it. Each line has a test.

| Threat | What happens | Defence | Test |
|---|---|---|---|
| **Wrong invoice** | A payment for one order settles a different, unpaid one | One deposit address per invoice, never reused; an output is only ever considered for the invoice owning its address | `a_payment_to_one_invoices_address_never_settles_another_invoice`, `two_invoices_with_one_amount_are_told_apart_by_address`, `each_invoice_gets_its_own_deposit_address_and_the_exact_amount`, `an_address_is_never_given_to_a_second_invoice`, `an_exhausted_address_pool_refuses_new_invoices_rather_than_reuse` |
| **Underpayment** | Customer pays less and expects the order to count | Not settled; kept as an unmatched record naming the invoice | `underpayment_is_recorded_against_its_invoice_and_settles_nothing` |
| **Overpayment** | Customer pays more; shop may credit the wrong amount | Not settled; kept as an unmatched record naming the invoice | `overpayment_is_recorded_and_never_settles` |
| **Double use** | One payment is used to pay two invoices, or one invoice twice | Each `txid:index` is claimed once, durably; a second payment to a paid invoice is recorded, not applied | `one_output_is_claimed_once_even_when_the_block_repeats`, `a_second_payment_to_a_settled_invoice_is_recorded_not_applied` |
| **Same-height competing branch** | The payment's block is replaced at the same height and the tip does not move | Each scan checks the stored block at the scan position against the node before deciding | `a_same_height_competing_branch_does_not_confirm` |
| **Reverted invoice reported as delivered** | A shop's 200 arrives after a reorg reverted the invoice | A result is recorded only against the same confirmed payment | `a_delivery_recorded_after_its_invoice_was_reverted_is_not_marked_delivered` |
| **Reorg after delivery** | The shop acted on a payment that a reorg removed | The invoice is flagged and listed at `GET /v1/alerts` (token needed), and an error is logged | `a_reorg_after_delivery_flags_the_invoice_for_the_merchant`, `alerts_list_invoices_whose_delivered_payment_was_reorged_out` |
| **Dust flood** | Many tiny payments to the merchant grow the records without bound | Unmatched records are capped at 1,000, oldest evicted and logged; the list is paged | `dust_to_the_merchant_cannot_grow_the_unmatched_records_without_bound`, `unmatched_payments_are_paged_and_the_page_size_is_capped` |
| **One client uses up reads** | A single client makes everyone else's status and QR reads fail | Reads are limited per client address (50 a second), with a global ceiling (500 a second) | `public_reads_are_budgeted_per_client_address`, `public_reads_have_a_global_ceiling_across_clients` |
| **Ledger rewrite each block** | Every block rewrites the whole ledger | Block ids live in their own sidecar file | `block_id_changes_do_not_rewrite_the_ledger` |
| **Reorg after payment** | The payment's block is replaced before it is final; the shop is told it was paid | Confirmation only at DAG-BFT finality, with the checkpoint matched to the scanned chain; a changed parent rolls back to pending and no webhook is sent | `a_reorged_unfinalized_payment_never_fires_the_webhook`, `a_reorged_payment_is_unsettled_and_never_confirmed`, `a_payment_on_the_replacement_branch_is_detected_from_that_branch`, `confirm_reverts_a_payment_whose_block_id_changed`, `only_finality_confirms_and_queues_the_webhook`, `a_checkpoint_on_another_branch_confirms_nothing`, `a_checkpoint_above_the_scanned_blocks_confirms_nothing`, `without_a_checkpoint_nothing_is_confirmed_or_announced` |
| **Webhook through a proxy** | A proxy resolves the shop's host and skips the address check | Proxies are disabled for webhook delivery | Reviewed, not tested offline |
| **Ledger read by others** | Invoice ids (read keys) and webhook URLs leak from the file | The ledger file is created 0600 on Unix and fsynced before the rename | `the_ledger_file_is_private_to_its_owner` (Unix only) |
| **Webhook body split into the nonce** | Bytes move between nonce and body and the signature still verifies | Nonce must be exactly 32 hex characters | `a_nonce_that_is_not_32_hex_is_refused_even_when_the_signature_verifies` |
| **Webhook storm** | Many shops fail together and retry in lockstep | Retry delay has up to a quarter of jitter | `retry_delays_are_jittered_so_shops_do_not_retry_in_lockstep` |
| **Stuck webhook** | Retries run out and the shop never hears | Resend endpoint for failed webhooks; delivered ones cannot be resent | `a_failed_webhook_can_be_resent_but_a_delivered_one_cannot` |
| **Dead node stops webhooks** | A node outage stops retries to shops | Delivery runs even when the scan fails | `a_failed_scan_does_not_stop_webhook_delivery` |
| **Slow node freezes the API** | A hung node holds the ledger lock | The tip is read before the lock is taken | `a_slow_node_does_not_hold_the_ledger_while_an_invoice_is_created` |
| **Old payment reused** | A payment made before the invoice existed is counted | Only blocks above the tip at creation count | `payment_in_or_before_the_creation_block_is_not_settled` |
| **Late payment** | A payment after expiry is counted | Only blocks up to the expiry height count | `payment_after_the_window_is_not_settled`, `expiry_happens_only_after_the_window_is_scanned` |
| **Restart forgets claims** | After a crash the same output settles a second invoice | Claims and scan position are committed together, atomically | `claims_survive_a_reopen`, `the_scan_position_survives_a_restart` |
| **Webhook spoofing** | Someone else sends your shop a "paid" message | HMAC-SHA256 with a 256-bit secret, compared in constant time | `spoofed_sender_without_the_secret_is_refused`, `a_correct_signature_verifies` |
| **Webhook tampering** | Body or headers changed in transit | The signature covers timestamp, nonce and body | `a_changed_body_is_refused`, `a_changed_timestamp_or_nonce_breaks_the_signature` |
| **Webhook replay** | A captured request is sent again | Timestamp window of 5 minutes, and a nonce cache on the receiver; retries keep one `event_id` to deduplicate on | `a_delivery_outside_the_tolerance_window_is_stale`, `a_replayed_nonce_inside_the_window_is_refused`, `a_replayed_webhook_is_refused_and_a_retry_is_deduplicable` |
| **SSRF via webhook URL** | The shop points the webhook at the service's own network or cloud metadata | HTTPS only; private, loopback, link-local, CGNAT and reserved addresses refused; connection pinned to the checked addresses; no redirects; checked again at every delivery | `private_ip_literals_are_refused_before_any_lookup`, `a_name_resolving_to_a_private_address_is_refused`, `local_and_internal_names_are_refused`, `only_https_is_accepted`, `a_private_webhook_destination_is_never_contacted`, `public_ip_table` |
| **Token guessing** | Someone creates invoices with a guessed token | Constant-time comparison; 256-bit minimum length | `creating_an_invoice_needs_the_bearer_token`, `short_secret_is_refused` |
| **Secret leak in logs** | A secret ends up in a log line | Manual `Debug` writes lengths only; secrets are zeroized | `debug_never_prints_a_secret` |
| **Invoice id guessing** | Someone reads another shop's invoice | 128 random bits per id | `an_unknown_invoice_is_not_found_and_a_known_one_is_public_without_secrets` |
| **Oversized requests** | Memory exhaustion | 8 KiB body limit, 2 KiB URL limit, 64-byte memo | `bad_amounts_and_memos_are_refused`, `overlong_urls_are_refused` |
| **Node lies about blocks** | A block out of order hides or invents a payment | Sequence checked before any change; the batch is refused whole | `a_block_out_of_sequence_changes_nothing` |
| **Node down** | Invoices issued at a guessed height | Creation stops with 503 when the node cannot say its tip | `an_unreachable_node_stops_creation_rather_than_guessing_a_height` |

**Known limits, stated plainly.**
- A checkpoint more than 256 blocks below the scan position cannot be matched,
  so confirmation waits until finality catches up. The checkpoint's signatures
  are not checked: the service trusts its node, as it does for every block.
- A webhook already delivered is not recalled if a reorg later removes its
  payment (that needs a finalized block replaced). The service logs it as an
  error and lists it at `GET /v1/alerts`; you must resolve it.
- A payment split across several outputs is not added together: each part is
  an underpayment.
- You must keep the address file topped up; addresses are never reused.
- The service does not limit how many open invoices you create. Limit open
  orders per customer in your own backend.
- Deliveries run one at a time. A slow shop delays the rest of the queue (not
  the scan). Upgrade path: bounded concurrency.
- The unmatched list has no assign endpoint yet: assignment is by hand.
- Ledger growth: invoices and claims are never pruned, because a pruned claim
  could let an old output settle again. Upgrade path: archive terminal records
  with their claims kept.
- The HTTP API is plain HTTP inside the service. Expose it only through a TLS
  proxy or a private network.
- No external security audit has been done. Maya2C does not have one yet, and
  this service has not been tested against a live shop.

## Design

[ADR-091](../../docs/adr/ADR-091-merchant-checkout.md) records the decisions:
the payment link format, the matching rule, and why the service is detached.
