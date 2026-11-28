# ADR-149: Maya2C Chat relay storage under flood

**Status:** Accepted, 2026-11-21. Built in `apps/chat/src/relay.rs` (PR #638).
Maya2C Chat stays RESEARCH ([ADR-031](ADR-031-maya-chat.md)).
**Date:** 2026-11-21
**Related:** [ADR-031](ADR-031-maya-chat.md) (relays, postage),
[ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) (outbox, receipts).

Nothing here touches the node, consensus or state. The node binary does not
link `maya-chat`. One wire change: `Request::Fetch` carries the challenge
nonce (decision 2), shipped with the fetch-domain v2 change of the same PR, so
relays and clients upgrade together once.

## Context

A relay cannot tell senders apart (ADR-031): addresses cost nothing and the
sender is inside the encryption. Postage (20 bits of work per deposit) is the
only price of a deposit, and a review of PR #638 measured what that buys:

- **Filling the relay.** 1 GiB (`MAX_RELAY_BYTES`) is about 4,096 envelopes of
  256 KiB, or 2^32 hashes in all: minutes on one CPU, seconds on a GPU. Before
  this ADR a full relay refused every deposit with "relay full" until the
  flood expired, up to `MAX_TTL` (30 days). Nothing evicted.
- **The challenge table.** Fetch challenges were free to ask for and held in a
  10,000-entry table. Eviction of the oldest entry (the previous fix for a
  table that refused when full) let a flood of 10,000 requests push out an
  owner's pending challenge, and each request in a full table scanned it.
- **Fetched ids.** A fetch freed an envelope's bytes but kept its id (in `seen`
  and the expiry index) until it expired, with no count limit, so a
  deposit-then-fetch loop grew memory outside `MAX_RELAY_BYTES`.

Scaling postage with envelope size was proposed (audit commit 40a1aa61) and
not taken: it changes the deposit rule on the wire, and with a sender's mint
cap of 24 bits (`MAX_POSTAGE_BITS`) a 256 KiB envelope (28 bits under that
proposal) could not be sent at all. Work alone cannot price out an attacker
with a GPU; what a relay controls is who pays when it is full.

## Decision

1. **At the cap, the largest mailbox gives way.** A deposit that would pass
   `MAX_RELAY_BYTES` evicts the oldest envelopes of the largest mailbox, while
   that mailbox holds more than the recipient's mailbox would after the
   deposit. When no mailbox is larger, the deposit is refused ("relay full").
   This is a max-min fair share of the relay, not protection from a flood: a
   flood of mailboxes larger than a victim's displaces its own envelopes
   first, but a flood of mailboxes smaller than a victim's (many fresh
   addresses, one envelope each) evicts the victim's oldest envelopes, down to
   the size of the flood's own mailboxes. An evicted envelope drew no
   receipt, so the sender's outbox (ADR-074) resends it; its id is forgotten so
   the resend is stored, not taken as a duplicate. A size index
   (`BTreeSet<(bytes, address)>`) finds the largest mailbox in O(log n).
2. **Stateless fetch challenges.** A challenge is `issued_at (8 bytes) || salt
   (8 random bytes) || tag (16 bytes)`, the tag a keyed BLAKE3 over a domain,
   the address and the first 16 bytes, under a per-process random key held in
   `Zeroizing`. The relay stores nothing on `Challenge`; the client echoes the
   nonce in `Fetch`. A flood of challenge requests costs no memory and cannot
   push out anyone's pending challenge. The tag is compared in constant time.
   Spent nonces are kept for `CHALLENGE_TTL` (120 s) to refuse a replayed
   fetch, at most `MAX_SPENT_CHALLENGES` (100,000), oldest forgotten first.
   Forgetting one reopens nothing a third party can use: the signature names
   this relay (fetch domain v2) and travels inside the Noise channel. A failed
   fetch spends nothing, so the owner's challenge stays usable (the property
   #582 added), and asking twice voids neither nonce.
3. **Fetched ids are bounded.** A fetched envelope's id moves to its own
   expiry-ordered set and is remembered so a resend after the fetch is not
   delivered twice, at most `MAX_FETCHED_IDS` (65,536), the soonest to expire
   forgotten first. A resend of a forgotten id is delivered again and the
   receiver's session refuses it as a replay (ADR-031, ratchet).

## Consequences

- A relay full of flood keeps taking mail for every mailbox smaller than the
  flood's largest. The flood's cost to squeeze every mailbox down to S bytes
  is about `MAX_RELAY_BYTES / S` stamps, and it must keep paying as the
  displaced mail is resent and its own envelopes are evicted in turn.
- **Accepted tradeoff: a flood can evict stored mail.** At the default 20
  stamp bits, about 4,096 stamps of 256 KiB fill the relay; after that each
  flood deposit into a fresh small mailbox evicts the oldest envelopes of the
  largest real mailboxes. Recovery is the sender's outbox resending until a
  receipt arrives; if the sender is offline or has uninstalled, the envelope
  is lost and nobody is told. Before this ADR the same flood left stored mail
  alone but refused every new deposit for up to `MAX_TTL`. A relay that keeps
  taking mail was judged the better failure; a floor that protects mailboxes
  up to a fraction of `MAX_MAILBOX_BYTES` was considered and not taken, since
  fresh addresses can sit under any floor. The real fix is size-aware postage
  (below; [BACKLOG.md](../../BACKLOG.md), "Chat size-aware postage").
- Restarting a relay changes the challenge key, so challenges issued before
  the restart are refused; the client asks again (the table was lost on
  restart before as well).
- `Relay::with_capacity` sets the byte cap, for tests and small relays.

## Not decided here

- **Size-aware postage.** Charging by size, or shortening the retention of
  large envelopes, would raise the price of holding the relay for long and of
  the eviction flood above; it is a wire and client change and stays open,
  within the 24-bit mint cap. Tracked in [BACKLOG.md](../../BACKLOG.md).
- **Identity-gated mailboxes** (proof of personhood, contacts-only): ADR-031
  lists them as the stronger answer; still later decisions.
- **Session table on the client.** The client's session table is unbounded,
  and each stranger handshake costs one stamp; an LRU design is tracked in
  BACKLOG.md.

## Tests

`apps/chat/tests/relay_bounds_tests.rs`:
`a_full_relay_takes_mail_for_a_smaller_mailbox_from_the_largest`,
`a_flooder_cannot_push_out_a_mailbox_no_larger_than_its_own`,
`an_evicted_envelope_is_stored_again_when_resent`,
`a_challenge_flood_does_not_push_out_the_owners_pending_one`,
`fetched_ids_are_remembered_only_up_to_the_cap`.
`apps/chat/tests/relay_tests.rs`: `a_spent_or_borrowed_challenge_is_refused`,
`a_second_challenge_does_not_void_the_pending_one`,
`a_failed_fetch_leaves_the_owners_challenge_usable`.
