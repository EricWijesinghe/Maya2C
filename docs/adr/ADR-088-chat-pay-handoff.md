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
policy engine.

The receiving account is its own signed field, `pay_to`, checked in one
place (`pay::check_pay_to`). **Superseded by amendment 1 (2026-11-13,
H2):** `pay_to` is the account of a wallet-signed binding the request
carries, never the chat identity's own address, so the chat holds no
spending key. "The signer's account" below now reads "the account the
signer's wallet bound it to".

### 2. Three messages, all inside the session AEAD

Each is a session plaintext with its own marker, like receipts (ADR-074).
The client refuses user text that starts with any marker, so typed text
cannot pose as a request or a receipt.

| Marker | Sent by | Carries | Signed by |
|---|---|---|---|
| `\0maya-chat pay v1\0` | payee | payee, `pay_to`, chain tag, expires, nonce, amount, memo | payee identity |
| `\0maya-chat paid v1\0` | payer | request id, tx id (the chain tag is signed, not sent) | payer identity |

The chat's own session already authenticates the sender, so the identity
signature is what makes a request **transferable evidence**: the signed
statement is what the payee keeps as the request record.

### 3. Request fields and limits

- `chain`: the 32-byte chain tag, the genesis block id that `get_chain_info`
  serves and chain signatures commit to (ADR-036). Signed in both the
  request and the receipt; a client refuses either for a chain other than
  the one set with `Client::set_pay_chain`, and the payee's node must report
  the same genesis before anything settles.
- `pay_to` (the `to` of the link): 32-byte chain address. Text form (`maya1…`, `tmaya1…`) only in the
  URI and the UI, decoded by `maya-address` with the expected network. A
  wrong-network address is refused by name (ADR-081).
- `amount`: decimal base units (ADR-046, 256-bit). Digits only, no leading
  zero, nonzero, at most 78 digits, and not above the U256 maximum. Kept as a
  string on the wire so the chat crate needs no big-integer dependency.
- `memo`: at most 140 bytes of UTF-8. No C0/C1 control characters, no
  Unicode format (`Cf`) character (bidirectional controls and isolates,
  zero-width and direction marks, U+061C, U+2060–U+2064, soft hyphen, tag
  characters), no line or paragraph separator, no variation selector and no
  Hangul filler. It is shown as text only (`textContent`), never as HTML,
  labelled as the payee's claim, and never rendered next to the amount,
  which the UI takes from the signed field.
- `amount` must also fit the link's 40-character whole-coin field, so every
  request that can be signed can be paid through a link.
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
the payee, the chain tag, the request id and the tx id. The payer's chat
sends it only for a request it received from that payee and has not already
marked paid (`Ledger::mark_paid`); it builds the wallet link only for a
received, unexpired, unpaid request (`Client::pay_link`).

On arrival the payee keeps the receipt only if the signature verifies
against the session peer's key and the request id is one it issued **to that
peer** and has not settled (`Ledger::check_receipt`). It then settles only
with chain evidence, and `Ledger::settle` checks every binding (PR #441
review, H1, M2, L4):

1. the receipt comes from the payer the request was issued to;
2. the evidence is for the tx id the receipt names;
3. the transaction's **sender** is the payer's wallet account, as the
   wallet binding in the payer's receipt names it (amendment 1). A payment
   from the payer's chat address does not settle;
4. the block is **above the tip height recorded at issue**
   (`queue_request` takes it), so an older transaction cannot pay a newer
   request, and a swept settled entry cannot be reused;
5. the block is **final**: at or below the newest quorum-attested checkpoint
   (`get_checkpoint`, ADR-038), the signal the explorer reads;
6. an output pays the request's `pay_to` at least the amount;
7. the tx id has not settled another request. Settled tx ids are kept for
   twice the 24-hour cap after settling, longer than any request issued
   before that transaction can stay open.

The evidence type (`pay::FinalTx`) is made only by `pay::chain::find_final`,
which asks the node for `get_chain_info` (genesis must equal the request's
chain), `get_checkpoint` and `get_block_by_height` (all three public through
the API gateway), and searches upward from one above the issue height, at
most `pay::MAX_SCAN_BLOCKS` per call, returning where to resume.

Trust: the node is trusted for what it reports; the checkpoint's validator
signatures are not verified by the chat. Point it at a node you run.

The chain has no memo field, so the request id is not carried in the
transaction. Binding is by sender, height window, amount and receiving
account. A payer who sends the payee two equal payments in the window can
name either; that is the payer's own money.

### 5. The URI contract: deep links carry prefill, never authority

Outbound (chat to wallet):

```text
maya2c-wallet://pay?to=<bech32m>&amount=<whole coins, decimal>&request=<64 lowercase hex>&chain=<64 lowercase hex>
```

Return (wallet to chat):

```text
maya2c-chat://paid?request=<64 lowercase hex>&tx=<64 lowercase hex>
```

The amount is in whole coins, as the wallet's route reads it (decimal, at
most `maya_amount::COIN_DECIMALS` places, nonzero, no leading zero before
the point). The memo and the expiry are not in the link: the memo is shown
on the chat card, and the expiry is enforced by the payer's chat when it
builds the link. The wallet must refuse a `chain` that is not its own
genesis id, and should refuse a `request` id it has already paid: chat
cannot stop the same link being opened twice outside it. The wallet's
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
Unit tests are in `apps/chat/src/pay/mod.rs` (`pay::tests`),
`pay/ledger.rs` (`ledger::tests`), `pay/chain.rs` (`chain::tests`) and
`pay_link.rs` (`pay_link::tests`); whole exchanges in
`apps/chat/tests/pay_tests.rs`.

| Threat | Control | Test |
|---|---|---|
| Spoofed request: payee swapped | payee signed; a changed payee fails | `pay::tests::an_address_swap_fails_the_signature` |
| Request names another account | `pay_to` must be the account of the payee's wallet binding (amendment 1) | `pay::tests::a_swapped_receiving_account_is_refused`, `pay_tests::a_payee_cannot_send_a_request_signed_for_another_account` |
| Forged wallet binding | hybrid signature by the key whose chain address is the account | `binding::tests::a_forged_binding_is_refused`, `pay::tests::a_request_with_a_forged_binding_is_refused`, `pay_tests::a_request_with_a_forged_binding_is_refused_on_arrival` |
| Binding of another chat identity | binding's chat id must be the signer | `binding::tests::a_binding_for_another_chat_identity_is_refused`, `pay::tests::a_request_with_another_chat_identitys_binding_is_refused`, `pay_tests::a_request_carrying_another_chat_identitys_binding_is_refused_on_arrival` |
| Binding for another chain | binding signs the genesis id | `binding::tests::a_binding_for_another_chain_is_refused`, `pay_tests::a_request_carrying_another_chains_binding_is_refused_on_arrival` |
| Expired, revoked or superseded binding | expiry at most 90 days; newest binding per chat id kept by each contact; revocation frame | `binding::tests::an_expired_post_dated_or_over_long_binding_is_refused`, `ledger::tests::a_superseded_or_revoked_binding_is_refused_once_a_newer_one_is_seen`, `pay_tests::a_revocation_reaches_contacts_and_old_requests_stop_being_payable`, `pay_tests::a_request_whose_binding_expired_is_refused_on_arrival` |
| Payment from the payer's chat address | sender must be the payer's bound wallet | `ledger::tests::a_payment_from_the_payers_chat_address_does_not_settle`, `pay_tests::a_payment_from_the_payers_chat_address_does_not_settle` |
| Wallet pays a chat request twice, or for another chain | wallet's paid list on disk; link chain must be the signing genesis | `chat_binding_tests::a_paid_request_cannot_be_paid_again_even_after_a_restart`, `chat_binding_tests::a_pay_link_for_another_chain_is_refused` (wallet core) |
| Forged or relayed sender | ML-DSA check against the session peer's key; sender must be the payee | `pay::tests::a_request_from_another_identity_key_is_refused`, `pay::tests::a_relayed_request_is_refused_when_its_sender_is_not_the_payee` |
| Replay of a request | ledger refuses a request id seen before | `ledger::tests::a_replayed_request_is_refused`, `pay_tests::a_replayed_envelope_is_refused_and_not_shown_twice` |
| Stale request | expiry checked; TTL capped at 24 h | `pay::tests::expired_and_too_long_requests_are_refused`, `pay_tests::an_expired_request_is_refused_on_arrival` |
| Amount tampering | amount signed; malformed amounts refused | `pay::tests::amount_tampering_fails_the_signature`, `pay::tests::malformed_amounts_are_refused` |
| Phishing text in memo | length and character rules; text only | `pay::tests::memo_with_controls_format_or_direction_marks_is_refused` |
| Request or receipt from another chain | chain tag signed; node genesis checked | `pay::tests::a_request_for_another_chain_is_refused`, `pay_tests::a_request_for_another_chain_is_refused_on_arrival`, `chain::tests::a_node_on_another_chain_is_refused` |
| Someone else's payment to the payee named as ours (H1) | tx sender must be the payer | `ledger::tests::someone_elses_payment_to_the_payee_does_not_settle`, `pay_tests::a_public_payment_to_the_payee_from_someone_else_does_not_settle` |
| Old tx reused for a new request | block must be above the issue height | `ledger::tests::a_transaction_from_before_the_request_does_not_settle`, `pay_tests::a_payment_from_before_the_request_is_not_searched_or_settled` |
| Unfinalized tx later dropped (M2) | block at or below the checkpoint | `ledger::tests::an_unfinalized_transaction_does_not_settle`, `chain::tests::a_payment_above_the_checkpoint_is_not_final_yet` |
| Replayed `paid` receipt | request settled once | `ledger::tests::a_final_payment_from_the_payer_settles_once` |
| Tx reused for two requests | txid recorded, kept 48 h after settling | `ledger::tests::one_transaction_cannot_settle_two_requests`, `ledger::tests::a_settled_transaction_is_remembered_longer_than_any_request_it_could_pay` |
| Payment short or to another account | output to `pay_to`, at least the amount | `ledger::tests::a_short_or_misdirected_payment_does_not_settle` |
| Receipt from another contact (L4) | receipt sender must be the request's payer | `ledger::tests::a_receipt_from_another_contact_does_not_settle`, `pay_tests::a_receipt_from_someone_other_than_the_payer_is_refused_on_arrival` |
| Payer pays twice, late, or unasked (M3) | payer ledger: received, unexpired, unpaid | `ledger::tests::the_payer_cannot_pay_twice_late_or_unasked`, `pay_tests::the_payer_cannot_pay_twice_or_pay_an_expired_request`, `pay_tests::a_receipt_for_a_request_never_issued_to_that_payer_is_refused` |
| Crash loses a receipt or an issued request (M4) | pay inbox and ledger saved with the sessions | `pay_tests::payment_state_survives_a_restart_before_it_is_taken` |
| One contact fills the ledger (M5) | 64 open entries per contact, inbox capped at 256 | `ledger::tests::one_contact_cannot_fill_the_ledger_for_everyone` |
| Deep-link injection | strict keys, duplicate and unknown keys refused, length cap | `pay_link::tests::a_memo_or_any_other_key_is_refused_not_ignored`, `pay_link::tests::wrong_scheme_spaces_and_oversized_links_are_refused` |
| Unparseable or non-canonical link amount (L2, L3) | canonical whole coins; signable implies linkable | `pay_link::tests::whole_coin_text_is_refused_when_the_wallet_would_refuse_it`, `pay_link::tests::every_amount_the_link_accepts_round_trips_and_larger_ones_are_refused` |
| User text posing as a request | markers refused in `queue` | `pay_tests::typed_text_cannot_pose_as_a_request_or_a_receipt` |
| Wrong-network address | `maya-address` decodes for the expected network | `pay_link::tests::a_pay_link_for_another_network_is_refused` |

## Consequences

- The chat gains two message types, a persisted ledger (issued requests with
  their payer, receiving account, amount and issue height; settled tx ids;
  received requests and whether they were paid) and a persisted inbox of
  opened payment frames. Bounds: `pay::MAX_LEDGER` per direction,
  `pay::MAX_PER_PEER` per contact, `client::MAX_PAY_INBOX` frames. Entries
  past their window are swept; nothing is evicted early to make room, a
  full ledger refuses instead.
- Save rule: the caller saves the client after every payment call and
  before it sends from the outbox or ends the pass.
- The payee's chat makes RPC reads at settlement time, not at send time,
  plus one tip read when it issues a request.
- The chat crate gains path dependencies `maya-address` and `maya-amount`,
  and `reqwest` (rustls) plus `serde_json` for the RPC, both already in the
  workspace.
- A chat user who never pays or requests is unaffected: no new traffic.

## Not done

- OS registration of `maya2c-wallet` and `maya2c-chat` schemes, and the
  deep-link plugin. ADR-083 requires its dependency review first.
- Refunds, partial payments, multiple payees, group requests.
- Verifying the checkpoint's validator signatures in the chat (decision 4,
  trust).
- A caller: no GUI calls `find_final` or `settle` yet.
- A wallet screen for bindings: the `chat_binding` command exists, the UI
  button does not; the chat-gui has no "paste binding" step yet.

## Amendment 1 (2026-11-13): the wallet signs a binding (H2)

Eric's decision on review finding H2: the chat must never hold a spending
key. Money is requested to, and paid from, wallet accounts only.

**The binding.** The wallet signs, with the account's own hybrid key (suite
`0x30`, ML-DSA-65 + SLH-DSA-SHA2-128s, the scheme every transaction uses):

```text
"maya2c wallet chat binding v1\0" ‖ kind (1 = bind, 2 = revoke) ‖ genesis id (32)
  ‖ chat identity (32) ‖ account (32) ‖ issued_at (u64 LE) ‖ expires_at (u64 LE)
```

On the wire it is the fields without the domain, then the 1,984-byte public
key, then the 11,165-byte signature (13,262 bytes). The account must be the
chain address of the key (`blake3("custom-l1-node.address.v3" ‖ pk)`), so a
binding is signed by the account it names and by nothing else. The domain
cannot be confused with a transaction's signing bytes, which begin with the
node's transaction domain. No new cryptography and no new dependency: the
chat verifies with `maya-crypto-pq` (already its dependency), the wallet
signs with the node's key code it already uses.

**Carried by both messages.** A request carries the payee's binding and
`pay_to` must be its account (`pay::check_pay_to`). A receipt carries the
payer's binding, and settlement requires the transaction's sender to be its
account (decision 4, point 3). Both bindings are inside the identity
signature's digest. Each is checked for: valid signature, account = key's
address, chat id = the message's signer, chain = the chain set for payments,
kind = bind, and the time rules below.

**Version, expiry, revocation.**

- `issued_at` is the sequence. Each contact's ledger keeps the newest
  binding seen per chat identity (`Ledger::observe_binding`) and refuses an
  older one, or a different one with the same sequence.
- Lifetime at most 90 days (`MAX_BINDING_TTL`, a policy bound; the wallet
  offers 30 by default), issue time at most 10 minutes ahead of the
  checker's clock. A binding nobody heard revoked still ends.
- A revocation is a binding of kind 2, signed the same way. The chat sends
  it to contacts as its own frame (`\0maya-chat binding v1\0`,
  `Client::queue_binding`); a contact that records it refuses requests from
  that identity, and refuses to build a pay link for a request it already
  holds, until a newer kind-1 binding arrives. A newer kind-1 binding to a
  new wallet supersedes the old one the same way.
- Records are kept until `issued_at + MAX_BINDING_TTL`: every older binding
  that still verifies has expired by then.

What revocation does not do: a contact that never receives the revocation
frame accepts the old binding until it expires. The owner's honest chat is
the only carrier, so the 90-day cap is the backstop. A thief holding only
the chat identity key can still direct payments only to the bound wallet,
which pays the owner.

**Wallet side.** `maya-wallet-core::chat_pay` builds the signed bytes from
the terms it shows (never from bytes the chat hands it) and refuses a
lifetime the chat would refuse. The Tauri command `chat_binding` shows the
chat identity, the account and the lifetime on the native confirm screen,
then returns the binding as hex for the chat (`Client::set_pay_binding`).
The `pay` route accepts `request` and `chain` (both or neither, 64 lowercase
hex each), refuses a request id in the wallet's paid list (kept on disk,
seven days), and `sign_transfer` refuses a `chain` that is not the genesis
it signs for, then records the id as paid.
