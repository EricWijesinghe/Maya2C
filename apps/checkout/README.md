# Maya2C checkout

Take payment in MAYA for an order. The checkout service makes a payment
link and a QR code for each order, watches the chain, and tells your shop
with a signed webhook once the payment is confirmed.

It is a separate program. It is not part of the node, and it never holds
your wallet's keys. It only needs your receiving address.

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

3. Your webhook receives one request when the payment is confirmed:

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

**Amounts that clash.** While an invoice for an amount is open (pending, or
paid but not yet confirmed), a second invoice to you for the exact same amount
is refused. Add or remove one base unit to make the amount different. This is
deliberate: the chain does not say which order a payment is for, so the exact
amount is what tells them apart.

**Only exact payments settle.**
- A payment of exactly the invoice amount settles it. Anything else does not.
- A customer who pays less, or more, does not pay the invoice. The invoice
  stays pending until it expires.
- Every other payment to you is kept as an *unmatched* record, which you can
  read with `GET /v1/unmatched` (bearer token). Nothing is credited for it.
  Refund it or assign it yourself.

**Failed webhooks.** If every retry fails, `POST /v1/invoices/<id>/resend`
(bearer token) puts it back in the queue once. A webhook already delivered
cannot be resent.

**Confirmations.** A payment counts after a number of blocks on top of it
(`MAYA_CHECKOUT_CONFIRMATIONS`, default 6). Raise it for larger orders.

**Expiry.** An invoice is open for `expires_in_blocks` blocks. A payment that
lands after that is ignored, and the invoice becomes `expired`.

## Configuration

| Variable | Required | Meaning |
|---|---|---|
| `MAYA_CHECKOUT_RPC` | yes | Node JSON-RPC URL |
| `MAYA_CHECKOUT_PAY_TO` | yes | Your receiving address (Bech32m text), on the node's network |
| `MAYA_CHECKOUT_API_TOKEN` | yes | Bearer token for creating invoices, at least 32 bytes |
| `MAYA_CHECKOUT_WEBHOOK_SECRET` | yes | HMAC key for webhooks, at least 32 bytes, different from the token |
| `MAYA_CHECKOUT_STORE` | yes | Absolute path of the ledger file. Back it up: it holds the record of which payments were used |
| `MAYA_CHECKOUT_LISTEN` | no | Default `127.0.0.1:8787` |
| `MAYA_CHECKOUT_CONFIRMATIONS` | no | Default 6, at most 256 |
| `MAYA_CHECKOUT_POLL_SECS` | no | Pause between scans, default 5 |

Secrets are read from the environment, held in memory that is wiped when it
is freed, and never printed or written to the log.

## Threat model

What an attacker can try, and what stops it. Each line has a test.

| Threat | What happens | Defence | Test |
|---|---|---|---|
| **Wrong invoice** | A payment for one order settles a different, unpaid one | Only an exact amount settles, and only if exactly one pending invoice fits | `first_pending_invoice_cannot_take_another_invoices_payment`, `two_pending_invoices_with_one_amount_settle_nothing` |
| **Underpayment** | Customer pays less and expects the order to count | Not settled; kept as an unmatched record for you | `underpayment_is_recorded_as_unmatched_and_settles_nothing` |
| **Overpayment** | Customer pays more; shop may credit the wrong amount | Not settled; kept as an unmatched record for you | `overpayment_is_recorded_as_unmatched_and_never_settles` |
| **Double use** | One payment is used to pay two invoices | Each `txid:index` is claimed once, durably. Two live invoices cannot share an amount | `one_output_is_claimed_once_even_when_the_block_repeats`, `a_claimed_output_cannot_settle_a_later_invoice`, `second_open_invoice_with_same_amount_is_refused`, `a_detected_invoice_still_holds_its_amount` |
| **Reorg after payment** | The payment's block is replaced; the invoice would still confirm | Block ids are stored; a changed parent rolls back; confirmation re-checks the block id | `a_reorged_payment_is_unsettled_and_never_confirmed`, `a_payment_on_the_replacement_branch_is_detected_from_that_branch`, `confirm_refuses_a_payment_whose_block_id_changed` |
| **Webhook through a proxy** | A proxy resolves the shop's host and skips the address check | Proxies are disabled for webhook delivery | Reviewed, not tested offline |
| **Ledger read by others** | Invoice ids (read keys) and webhook URLs leak from the file | The ledger file is created 0600 on Unix and fsynced before the rename | `the_ledger_file_is_private_to_its_owner` (Unix only) |
| **Webhook body split into the nonce** | Bytes move between nonce and body and the signature still verifies | Nonce must be exactly 32 hex characters | `a_nonce_that_is_not_32_hex_is_refused_even_when_the_signature_verifies` |
| **Webhook storm** | Many shops fail together and retry in lockstep | Retry delay has up to a quarter of jitter | `retry_delays_are_jittered_so_shops_do_not_retry_in_lockstep` |
| **Stuck webhook** | Retries run out and the shop never hears | Resend endpoint for failed webhooks; delivered ones cannot be resent | `a_failed_webhook_can_be_resent_but_a_delivered_one_cannot` |
| **Dead node stops webhooks** | A node outage stops retries to shops | Delivery runs even when the scan fails | `a_failed_scan_does_not_stop_webhook_delivery` |
| **Slow node freezes the API** | A hung node holds the ledger lock | The tip is read before the lock is taken | `a_slow_node_does_not_hold_the_ledger_while_an_invoice_is_created` |
| **Read flood** | Public reads starve the scanner | Public reads share a per-second budget (50) | `public_reads_are_budgeted_per_second` |
| **Old payment reused** | A payment made before the invoice existed is counted | Only blocks above the tip at creation count | `payment_in_or_before_the_creation_block_is_ignored` |
| **Late payment** | A payment after expiry is counted | Only blocks up to the expiry height count | `payment_after_the_window_is_ignored`, `expiry_happens_only_after_the_window_is_scanned` |
| **Restart forgets claims** | After a crash the same output settles a second invoice | Claims and scan position are committed together, atomically | `claims_survive_a_reopen`, `the_scan_position_survives_a_restart` |
| **Webhook spoofing** | Someone else sends your shop a "paid" message | HMAC-SHA256 with a 256-bit secret, compared in constant time | `spoofed_sender_without_the_secret_is_refused`, `a_correct_signature_verifies` |
| **Webhook tampering** | Body or headers changed in transit | The signature covers timestamp, nonce and body | `a_changed_body_is_refused`, `a_changed_timestamp_or_nonce_breaks_the_signature` |
| **Webhook replay** | A captured request is sent again | Timestamp window of 5 minutes, and a nonce cache on the receiver | `a_delivery_outside_the_tolerance_window_is_stale`, `a_replayed_nonce_inside_the_window_is_refused` |
| **SSRF via webhook URL** | The shop points the webhook at the service's own network or cloud metadata | HTTPS only; private, loopback, link-local, CGNAT and reserved addresses refused; connection pinned to the checked addresses; no redirects; checked again at every delivery | `private_ip_literals_are_refused_before_any_lookup`, `a_name_resolving_to_a_private_address_is_refused`, `local_and_internal_names_are_refused`, `only_https_is_accepted`, `a_private_webhook_destination_is_never_contacted`, `public_ip_table` |
| **Token guessing** | Someone creates invoices with a guessed token | Constant-time comparison; 256-bit minimum length | `creating_an_invoice_needs_the_bearer_token`, `short_secret_is_refused` |
| **Secret leak in logs** | A secret ends up in a log line | Manual `Debug` writes lengths only; secrets are zeroized | `debug_never_prints_a_secret` |
| **Invoice id guessing** | Someone reads another shop's invoice | 128 random bits per id | `an_unknown_invoice_is_not_found_and_a_known_one_is_public_without_secrets` |
| **Oversized requests** | Memory exhaustion | 8 KiB body limit, 2 KiB URL limit, 64-byte memo | `bad_amounts_and_memos_are_refused`, `overlong_urls_are_refused` |
| **Node lies about blocks** | A block out of order hides or invents a payment | Sequence checked before any change; the batch is refused whole | `a_block_out_of_sequence_changes_nothing` |
| **Node down** | Invoices issued at a guessed height | Creation stops with 503 when the node cannot say its tip | `an_unreachable_node_stops_creation_rather_than_guessing_a_height` |

**Known limits, stated plainly.**
- Reorgs deeper than the last 256 blocks are not detected. Settlement waits for
  the confirmation depth you set, and a checkpoint anchor is the planned upgrade.
- A webhook already delivered is not recalled if a reorg later removes its
  payment. The service logs it as an error; you must resolve it.
- A payment split across several outputs is not added together.
- The chain has no memo, so the exact amount is the link between a payment and
  an invoice. That is why live amounts must differ for each recipient.
- Payments to you before your first invoice exists are recorded as unmatched
  only if `MAYA_CHECKOUT_PAY_TO` is set, which it must be.
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
