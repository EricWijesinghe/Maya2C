# ADR-074: Chat sender outbox, signed delivery receipts, envelope v2

**Status:** Proposed (2026-11-01). Ledger item C4. Extends ADR-031 (Maya Chat,
Accepted, RESEARCH: unaudited). Nothing here is built.
**Date:** 2026-11-01

## Context

Chat has no sender-side guarantee. A message is sealed, deposited once, and
forgotten.

- `apps/chat/src/courier.rs:42-69` `deliver` asks for a bundle, asks for
  postage, seals, mints, deposits, and returns. Nothing is stored locally
  before the network call, and nothing is retried.
- `apps/chat/src/relay.rs:219-258` `deposit` refuses on a full mailbox,
  postage below `STAMP_BITS` (`relay.rs:42`), or expiry outside `MAX_TTL`
  (`relay.rs:26`). A refusal means the relay does not hold it. No receipt
  exists, so the sender cannot tell "held by the relay" from "delivered".
- ADR-031 ("Delivery") says envelopes are idempotent by id, so resending is
  safe. That property is not used: no code resends.
- The chat-gui send path has no outbox. `apps/chat-gui/src-tauri/src/lib.rs:226-249`
  `send` reports an error and drops the message. Stickers share the same
  shape: `sticker_cmds.rs:159-167`.

**A safety bug on the send path, found while writing this ADR.** The
ratchet state is rewound on a failed send, and a rewound ratchet reuses keys.

- `lib.rs:238` (inside `send`) and `sticker_cmds.rs:163` (inside `deliver`)
  reload the session from disk when `courier::deliver` returns an error:
  `*s = App::session(&app.files, s.seed.clone())?`.
- `client.seal` advances the send chain in memory before the deposit
  (`apps/chat/src/session.rs:241-258` `encrypt`: the message key is derived from
  `send_chain`, then the chain steps, then `send_n` increments).
- The state is saved only after success (`lib.rs:249` `app.save(s)`).
- If the relay stored the envelope and the reply was lost (the request
  timeout is 30 s, `net.rs:25`), the failed send is rewound to the last saved
  state. The next message uses the same `send_n`, so the same message key,
  and the AEAD nonce is `n` alone (`session.rs:187-189`). Two plaintexts under
  one ChaCha20-Poly1305 key and nonce expose the XOR of the plaintexts and
  allow forgery of the Poly1305 tag.
- The code comment at `lib.rs:236-237` acknowledges the rewind ("the sending
  chain may have moved past a message the relay never took") but does not
  treat the stored-but-unacknowledged case.

This is reproducible in principle with a relay that stores then times out. It
has not been reproduced on a live relay. The decision below fixes it
regardless.

Other facts the design uses:

- Envelope bytes are CBOR via ciborium (`relay.rs:77-81`). `Vec<u8>` fields are
  written as CBOR integer arrays, so ciphertext costs about two bytes per byte
  (`BACKLOG.md:67-72`; a 120 KiB sticker seals to 247,427 B, from the sticker
  work in `features.toml`).
- Envelope ids exclude the stamp (`relay.rs:89-94`), so a re-minted stamp keeps
  the id. The relay's `seen` map dedupes by id (`relay.rs:219-224`).
- Limits: `MAX_ENVELOPES` 1,000 and `MAX_MAILBOX_BYTES` 16 MiB per mailbox
  (`relay.rs:20-22`), `MAX_ENVELOPE_BYTES` 256 KiB (`relay.rs:24`),
  `MAX_RELAY_BYTES` 1 GiB (`relay.rs:31`), `ENVELOPE_TTL` 7 days
  (`client.rs:18`).
- Identity signatures are ML-DSA-65 (`identity.rs:83` `sign`, `identity.rs:152`
  `verify` via `suite::verify`). A signature is about 3.3 KB (ADR-031,
  Consequences).
- Session replay and out-of-order handling: `session.rs:260-292` `decrypt`,
  `MAX_SKIP` 1,000 (`session.rs:22`).
- Client state is saved encrypted under a seed-derived key (`client.rs:74-94`
  `save`, `client.rs:64-67` `state_key`). It has no version field
  (`client.rs:36-39`).

## Decision

### 0. Fix the rewind first (separate PR, before any feature)

- A failed deposit **never reloads or rewinds the session.** The in-memory
  state stays as sealed. The session is saved to disk before the envelope is
  sent, so a crash or lost reply cannot reuse a step.
- Sealing is split from sending. The sealed envelope is the unit of work.
  Retries resend the same bytes; they never reseal (decision 1).
- Regression test: after a deposit whose reply is dropped, the next seal uses a
  different message key and nonce (`n` strictly increases across a restart).

### 1. Persistent sender outbox

- The outbox stores sealed `Envelope` bytes keyed by envelope id, with the local
  message id, the peer, the first-attempt time, the attempt count and the next
  attempt time. It lives in the existing encrypted state (`client.rs:36-39`
  `State`), so it uses the same key and `write_atomic` (`store.rs:138`).
- Write order: seal, persist outbox and session, then the network call. A
  crash between persist and send leaves an unsent entry, which the next run
  sends. It never leaves a sealed-but-unrecorded message.
- Retry: exponential backoff from 5 s, doubling, capped at 10 min, with ±20%
  jitter from `getrandom`. A retry resends identical bytes. Each attempt
  starts with `Postage`: if the relay now asks for more stamp work, the
  stamp is re-minted, which keeps the id (`relay.rs:89-94`).
- An entry leaves the outbox on a valid receipt (decision 2). A deposit
  accepted by the relay is shown as "relayed", not "delivered", until a receipt
  arrives.
- Entries older than `ENVELOPE_TTL` (`client.rs:18`) are marked `expired` and
  shown to the user. They are not deleted silently.
- Storage limits: 1,000 outbox entries, 16 MiB of sealed bytes, 256 KiB per
  entry (matching `relay.rs:20-24`). A send over a limit is refused with
  "outbox full: N undelivered". Old entries are never evicted to make room.

### 2. Signed delivery receipts

- A receipt is a new `Payload` variant (`relay.rs:46-57`):
  `Receipt { envelope_ids: Vec<[u8; 32]>, signature: Vec<u8> }`, sent as a
  normal session message (`Payload::Chat`, `session.rs:78-85`). It is encrypted
  like any message, so the relay cannot tell a receipt from a chat message
  except by size.
- The signed statement is the BLAKE3 digest, under domain
  `maya-chat 2026-11 receipt v1`, of: session id, recipient address, sender
  address, and each envelope id. The recipient signs it with its ML-DSA-65
  identity key (`identity.rs:83`).
- Verification on the sender: the signature is checked against the peer's
  public key, and `address_of(key) == peer` (`identity.rs:21`), before any
  state changes. An id not in the outbox is ignored. The outbox entry is
  removed once.
- A receipt means the recipient's device opened and authenticated the
  envelope. It does not mean a person read it. The UI says "delivered to
  device".
- Receipts are not themselves receipted. Receipts are batched: one per
  collection pass, carrying every id opened in that pass.

**Replay and forgery.**

- Replay of the ciphertext: the session refuses it (`session.rs:260-268`,
  `Replayed`).
- Replay of the signed statement inside a fresh message: the sender's outbox
  has no entry for that id any more, so it is ignored.
- Forgery: needs the recipient's ML-DSA-65 key. A relay cannot forge a receipt
  because it cannot read or create session messages.
- Cross-session: the signed statement includes the session id, so a receipt
  for one conversation cannot be moved to another.
- The signature is checked before the outbox is touched.

### 3. Receipts leak online status: opt-out

- Default: receipts **on**. Without them the outbox cannot tell a
  delivered message from a stuck one, and the sender retries until expiry.
- Settings: a global "send delivery receipts" switch and a per-contact switch.
  Either set to off means no receipts are sent for that scope. A receiver that
  turns receipts off still receives everything.
- Timing: a receipt is sent after the collection pass plus a random 0–30 s
  delay, so the receipt does not show the exact moment the app was opened.
- A sender whose contact has receipts off sees "relayed" and keeps retrying
  until expiry. The UI says why.

### 4. Envelope v2 with byte strings and a version

- `serde_bytes` on every `Vec<u8>` ciphertext, key, signature and ciphertext
  field: `Message.ciphertext` (`session.rs:78-85`), `Handshake` fields
  (`session.rs:41-54`), `PrekeyBundle` fields (`identity.rs:119`).
- A version field on `Envelope`: `#[serde(default)] version: u8`. A missing
  field decodes as version 1. Version 2 uses byte strings. The decoder reads
  both shapes; it does not guess (it branches on `version`).
- **Signatures do not change.** Signed bytes are built from raw bytes with
  `put()`, not from CBOR (`session.rs:57-66`, `identity.rs:138-150`). Existing
  sessions, handshakes and prekey bundles stay valid across the change.
- **Envelope ids change for v2 envelopes.** The id is a hash of the canonical
  encoding (`relay.rs:89-94`). The relay computes ids from the bytes it
  receives, so v1 and v2 dedupe separately, which is correct.
- Negotiation: a new `Request::Version` (`net.rs:29-50`) returns the highest
  envelope version the relay accepts. A client sends v2 only after the relay
  says so. The relay accepts both versions.
- Client state gets the same treatment: `State` (`client.rs:36-39`) gains a
  version, and a version-1 state loads into the current code.
- The sticker cap is not changed by this ADR. The 120 KB cap stays until a
  measurement, from the existing test `the_largest_sticker_crosses_real_encryption_within_one_envelope`
  (`apps/chat-gui/src-tauri/src/stickers.rs:368`), shows the v2 size. The
  number comes from that test's output, not from this ADR.

## Alternatives considered

- **Unsigned acknowledgements.** Forgeable by a relay that can see the
  envelope id. Rejected.
- **MAC instead of signature** (deniable receipts). Deniability would let the
  sender forge a receipt, which is the wrong direction for a delivery proof.
  Rejected for v1. A MAC could be added as a separate, later option.
- **Receipts in plaintext** on the relay. Leaks online status to the relay for
  every message. Rejected; receipts ride inside the session.
- **Resend by resealing** on retry. This is the nonce-reuse path above.
  Rejected outright.
- **Evict oldest outbox entries when full.** Silent loss of a message is the
  failure the outbox exists to prevent. Rejected; a full outbox refuses new
  sends instead.
- **Receipts off by default.** Makes "delivered" unknowable by default.
  Rejected; opt-out keeps the guarantee and gives the privacy choice.
- **Keep CBOR int arrays and raise the sticker cap.** Doubles every message
  and every relay's storage for no reason. Rejected.
- **Envelope v2 by renaming fields.** Does not fix the byte encoding, and
  breaks v1 decoding without a version to branch on. Rejected.

## Consequences

- A sender now holds unacknowledged ciphertext on disk. That is the same
  exposure as the session state already on disk, under the same key, and the
  file is written atomically.
- Receipts add roughly 3.4 KB per batch (ML-DSA-65 signature plus ids). That
  is small against a 256 KiB envelope. It is still a real cost on mobile
  networks.
- Receipts reveal that the receiver's device is online and has opened the
  app. The opt-out and random delay reduce this; they do not remove it. The
  threat model says so.
- Two envelope versions must be read for at least one release. Removing v1
  is a later decision, after relays and clients have upgraded.
- The rewind fix changes the behaviour of a failed send: the message is no
  longer reported as an error to a user whose relay stored it. The UI must say
  "relayed, waiting for receipt" in that case.
- Outbox retries use the network while the app is open. Background delivery
  on mobile is a separate question (out of scope).

## Security model

| Threat | Control |
|---|---|
| Key and nonce reuse after a lost reply | decision 0: no rewind; state saved before send; retries resend identical bytes; test |
| Relay drops a message silently | outbox with receipts; expiry is visible; no silent eviction |
| Relay forges a delivery receipt | receipts are session messages and signed by the recipient's ML-DSA-65 key; verified before state changes |
| Replay of a receipt | outbox removes the id once; unknown ids ignored; session replay refusal |
| Receipt moved to another conversation | session id inside the signed statement |
| Online-status leak | opt-out (global and per contact); random 0–30 s delay; batched receipts; documented residual leak |
| Storage exhaustion by a sender | outbox limits; refuse over limit; postage on every deposit (`relay.rs:42`) |
| Malformed v2 envelope | version branch; bounded sizes (`MAX_ENVELOPE_BYTES`); decode errors dropped per envelope, as `courier::collect` already does |
| Downgrade to v1 by an attacker | v1 is the old format with the same security properties; negotiation only chooses the format, not the cipher |
| Deniability | receipts are non-repudiable by design; stated to users in the receipt setting |

## Test plan

Each item must fail when the feature is broken.

1. **Rewind regression (unit, `apps/chat/tests/session_tests.rs`).** Seal;
   simulate a deposit that the relay stored with a lost reply; restore the
   saved state; seal again. Assert the message keys differ and `n` did not
   repeat. Fails on the current code at `lib.rs:238` / `sticker_cmds.rs:163`.
2. **Byte-identical retry (unit).** Retry an outbox entry twice; the deposited
   envelope bytes are equal, and the envelope id is equal.
3. **Crash between persist and send (unit).** Persist the outbox, drop the
   client without sending, restore; the entry is sent once with the same id.
4. **Backoff schedule (pure, unit).** Attempts at 5, 10, 20 ... capped at 600 s,
   each within ±20%; no attempt before its next time.
5. **Limits (unit).** 1,001st entry refused with the "outbox full" error; old
   entries untouched; 256 KiB entry refused; no silent eviction.
6. **Receipt verify (unit).** Valid receipt accepted; wrong key, wrong session
   id, wrong envelope id, and a receipt from a non-peer address all refused
   before the outbox changes; duplicate receipt is a no-op.
7. **Opt-out (unit).** Global off and per-contact off each produce zero
   receipts across a collection pass.
8. **Receipt batching and delay (pure, unit).** One receipt per pass; delay
   within 0–30 s.
9. **Envelope v1 and v2 golden vectors (unit).** No vectors exist today
   (`apps/chat/tests/` has none). First, generate v1 vectors from the current
   encoder and commit them, before any change. Then decode them after the
   change, and round-trip v2 and compare bytes. A v1
   sender and a v2 receiver interoperate in `net_e2e_tests.rs`.
10. **Size measurement.** Record the sealed size of a 120 KiB sticker and of a
    1 KiB text message under v1 and v2, using the existing sticker test
    (`stickers.rs:368`), and paste the output into `reports/`. The ADR does
    not state the result.
11. **Relay drop chaos (integration).** A relay that stores then drops replies
    at 30%; every message is delivered exactly once and receipted; no session
    rewinds (checked by counting message keys).
12. **Fuzz.** Envelope decoder over both versions, with a corpus in `fuzz/`.

## Rollout

1. **Rewind fix** (decision 0), its own PR, first. It is a security fix to
   shipped code and does not wait for the feature work.
2. **Relay accepts v1 and v2, and `Request::Version`** (decision 4). Relay
   upgrades first.
3. **Client v2 envelopes** behind negotiation; v1 stays for one release.
4. **Outbox** with "relayed" and "expired" states, and the UI for them.
5. **Receipts**, default on, with the opt-out settings and the delay.
6. Each step: `features.toml` entry for `maya-chat` updated in the same
   change (the chat row says "Unaudited"; that stays until an external
   review, which is a later decision), and the chat page and Universe text
   updated if the status changes (feedback memory, 2026-10-29). Stickers and
   chat stay RESEARCH until audited (ADR-031).

## Revisit when

- Background delivery on mobile is decided (outbox semantics change).
- Measured receipt traffic on a real relay exceeds the budget the team sets.
- Groups land (MLS), which needs receipts per member and a different signed
  statement.
- A review finds the session design needs a KEM ratchet (ADR-031 defers
  it); outbox state would then need to hold keys differently.
- Measured v2 size shows that the sticker cap should change; that is a
  product decision with its own record.
