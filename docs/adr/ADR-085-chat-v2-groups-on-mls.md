# ADR-085: Group chats on MLS with a hybrid post-quantum ciphersuite (chat-v2)

**Status:** Proposed, 2026-11-10. Design only; nothing is built. Chat stays
RESEARCH and unaudited ([ADR-031](ADR-031-maya-chat.md)). Decision D2b needs Eric's approval before any
group message reaches users.
**Extends:** [ADR-031](ADR-031-maya-chat.md) (which defers group chat to a later ADR), [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) (outbox,
receipts, envelope v2). Fulfils the CATALOG `chat-v2` gate.

## Context

Chat is two-party only today.

- `apps/chat/src/session.rs` (`encrypt` ~270, `decrypt` ~289): one symmetric
  sending chain per direction; no KEM ratchet, no re-key. Post-compromise
  recovery is not implemented ([ADR-031](ADR-031-maya-chat.md) "Re-keying" is a sketch).
- `apps/chat/src/relay.rs` (`Payload` ~62, `Envelope` ~76): `Open` and `Chat`
  only; `Envelope.to` is a single `Address`.
- `relay.rs` `fetch` (~340) empties a mailbox keyed by one address. A shared
  account mailbox would let one device drain every other device's messages, so
  multi-device needs one mailbox per device.
- `apps/chat/src/identity.rs` (`address_of` :21, `sign` :83): chat identity is
  ML-DSA-65 only, while invariant 30 names suite `0x30` (ML-DSA-65 +
  SLH-DSA-128s) as the hybrid. That is a pre-existing gap, recorded here, not
  fixed here.
- `relay.rs:20-24`: `MAX_ENVELOPES` 1,000, `MAX_MAILBOX_BYTES` 16 MiB,
  `MAX_ENVELOPE_BYTES` 256 KiB.
- [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) decision 0 (the rewind fix) is a prerequisite: a rewound MLS
  generation would reuse a message key and nonce.

## Research findings (2026-11-10)

- **F1 RFC 9420** sections used: §5.1 suites, §5.3 credentials, §7.2-7.3 leaf
  nodes, §8 key schedule (§8.4 PSK, §8.7 epoch authenticators), §9 secret tree
  and §9.2 deletion schedule, §12.1.3 Remove, §12.1.6 ExternalInit, §12.4
  Commit, §13.1 extra suites, §16.6 FS/PCS, §16.9-16.10 DS/AS compromise, §16.12
  insider fragmentation. Private-use code points `0xF000-0xFFFF`.
- **F2 openmls** latest on crates.io 0.9.2; feature `draft-ietf-mls-pq-ciphersuites`
  enables PQ suites in the libcrux and rust-crypto providers; MIT. Source read
  was `main`, not the 0.9.2 tarball; audit status not verified.
- **F3 X-Wing suites in openmls `main`** (draft feature): `0x004D`
  `MLS_256_XWING_CHACHA20POLY1305_SHA256_Ed25519` (doc says draft-01, mapping
  uses `XWingKemDraft6` — unresolved); `0x004E`
  `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`; `0x004F`
  `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519`; and an X-Wing + ML-DSA-44 suite.
- **F4** No shipped suite pairs X-Wing with ML-DSA-65 or a composite signature.
  `SignatureScheme` has ML-DSA-44/65/87 and Ed25519; no SLH-DSA, no composite.
- **F5** libcrux maps `XWingKemDraft6` to hpke-rs `XWingDraft06`, with a KAT
  against draft-ietf-hpke-pq.
- **F6** draft-ietf-mls-pq-ciphersuites is an active Internet-Draft; code points
  are provisional.
- **F7** `crates/crypto-pq` uses `x-wing = "=0.1.1"` tested against the draft's
  vectors (`src/kem_suite/xwing.rs`); its draft revision vs openmls's is unconfirmed.

## Decision

### D0. Pin the library
`openmls = "=0.9.2"` in `apps/chat/Cargo.toml` with the PQ-draft and libcrux
features (verify the feature names in the 0.9.2 manifest). Exact pin; a bump is
an ADR amendment. Confirm the node does not link it
(`cargo tree -p maya-chat -i openmls`). `cargo deny` and `cargo vet` entries. A
test asserts the numeric code point so an upstream renumbering fails CI.

### D1. Ciphersuite
`0x004E` `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`: X-Wing is the hybrid
KEM [ADR-031](ADR-031-maya-chat.md) already chose; AES-256-GCM and SHA2-384 are the strongest symmetric
options listed. `0x004D` rejected until its draft mismatch is resolved; `0x004F`
rejected (AES-128 for group secrets); ML-DSA-44 suite rejected (below the
identity level). The Ed25519 signature leg is a deviation; see D2.

### D2. Hybrid authentication
- **D2a (adopted, interim).** Each device holds a suite-`0x30` hybrid device key
  that co-signs (1) the device certificate (D3) and (2) every Commit over its
  confirmed transcript hash. Receivers check both the MLS signature and the
  co-signature before applying a Commit. ~11 KB per Commit from FIPS 204/205
  sizes; measure in the build.
- **D2b (OPEN — Eric decides).** Application messages under D2a are
  authenticated by Ed25519 only. Options: (i) reviewed fork or upstream change
  adding a composite scheme (ML-DSA-65 + SLH-DSA-128s to match `0x30`), using
  the private-use range until an upstream code point exists — recommended end
  state, cost is maintaining a crypto-library fork; (ii) per-message `0x30`
  co-signature (~11 KB/message) — short-term only; (iii) ship single-leg and
  disclose — rejected, breaks the hybrid rule. **No group message reaches users
  until D2b is decided.**

### D3. Multi-device as MLS leaves
One leaf per device; only the account identity is shared, and it only signs
device certificates.

```
DeviceCert {
  domain:        "maya-chat 2026-11 device cert v1"
  account:       Address        // address_of(account ML-DSA-65 key)
  device_id:     [u8; 16]       // getrandom
  mls_sig_key:   Vec<u8>        // Ed25519 (suite)
  hpke_init_key: Vec<u8>        // X-Wing public (suite)
  mailbox:       Address        // device mailbox key address
  hybrid_key:    Vec<u8>        // device 0x30 public key
  not_after:     u64
  account_sig:   Vec<u8>        // ML-DSA-65 account key over all fields above
  hybrid_sig:    Vec<u8>        // 0x30 device key over all fields above
}
```

- The MLS `BasicCredential` identity is the encoded `DeviceCert`.
- A member is accepted only if both signatures verify, the account matches,
  `not_after` is in the future and the device is not revoked — checked on Add
  before the Commit applies (`apps/chat/src/group/validate.rs`).
- Pairing: the new device shows a short authentication string
  (BLAKE3, domain `maya-chat 2026-11 pairing sas v1`) compared out of band; the
  primary signs the certificate. The account seed is never transferred.
- Revocation: Remove of every leaf of the device; expiry bounds a missed Remove.
- Losing every device loses the account; chain-side recovery ([ADR-056](ADR-056-guardian-recovery-transactions.md) / ADR-086)
  is the candidate, not decided here.

### D4. Delivery
- `Payload::Mls(Vec<u8>)` in `relay.rs` carries Welcome / Commit / PrivateMessage.
- `Envelope.to` is a device mailbox; each device fetches its own mailbox with
  the existing signed challenge.
- Fan-out: one ciphertext deposited per recipient device, each its own envelope id.
- Commit ordering: the Commit envelope carries `(group_tag, epoch)` with
  `group_tag = BLAKE3("maya-chat group tag v1" || group_id)`. The relay accepts a
  Commit for epoch `e` only if its head is `e-1`. An availability hint, not a
  trust point.
- Group payloads use [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) decision 4 byte strings; envelope version 3,
  negotiated via `Request::Version`.
- Handshakes sent as PrivateMessage so the relay does not see Add/Remove
  (openmls wire-format policy to verify).
- Forks: epoch gap or unknown head → resync by ExternalInit or re-invite;
  evaluate openmls `fork-resolution` before relying on it.

### D5. Attachments
Fresh 256-bit key per file; fixed-size chunks, ChaCha20-Poly1305, nonce = chunk
index, AAD = `file_id || index || chunk_count`; BLAKE3 root in the message.
Ciphertext chunks in a content-addressed relay blob store (upload once, not per
device: a 10 MiB file to 50 members × 2 devices would otherwise be 1 GB).
Sizes padded to power-of-two buckets; metadata stripped before encryption.
`MAX_ATTACHMENT_BYTES`, group cap and chunk size are set from a measured relay
budget — no value ships until measured. Rendering is sandboxed.

### D6. Offline and outbox
Reuse the [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) outbox: seal once, persist outbox and MLS state, then send;
retry resends identical bytes; resealing is forbidden. openmls `StorageProvider`
implemented over the encrypted client state (`client.rs` `save`,
`store.rs` `write_atomic`) in `apps/chat/src/group/store.rs`. Commits applied in
epoch order; a gap triggers resync. Group receipts are a later step.

### D7. Removal and post-compromise security
Remove + fresh path; deletion schedule (§9.2). PCS holds only after a Commit
with an update path the attacker did not see; a stolen device state reads until
then. Devices commit an Update on pairing, revocation and a timer (value set by
measurement). The account key is the root of trust and cannot be revoked by
group action alone.

### D8. Metadata leakage

| Observer | Learns | Does not learn |
|---|---|---|
| Relay | device mailbox addresses, deposit timing and fan-out count, blob sizes (bucketed), `group_tag` | plaintext, sender, membership changes |
| Relay over time | group size, activity pattern, device linkage if mailboxes are linkable | content |
| Member | other members' device certificates and the membership | messages outside the group |

Application payloads are padded to buckets before sealing. `group_tag` linkage
is a documented residual. Mixnet routing ([ADR-031](ADR-031-maya-chat.md)) is the real fix; deferred.

### D9. Migration from 1:1
Existing sessions keep working; nothing is re-encrypted. A "group upgrade"
creates a two-member group from the peer's device certificates; the 1:1 session
stays read-only for history. Peers without device certificates stay on 1:1 and
the UI says so. Client state version 3 loads versions 1 and 2.

## Threat model

| Threat | Control | Residual |
|---|---|---|
| Harvest now, decrypt later | X-Wing hybrid KEM (`0x004E`) | X-Wing / ML-KEM must hold |
| Quantum forgery of a Commit | Ed25519 + `0x30` co-signature (D2a) | — |
| Quantum forgery of an application message | Ed25519 only | **Open: D2b** |
| Forged device certificate | account + device hybrid signatures | account key compromise |
| Account key theft | key stays on primary, typed, zeroized | chain-side rotation undecided |
| Relay forks a group | head-epoch rule + resync | DoS / resync load |
| Relay drops/delays | [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) outbox + receipts | DoS only |
| Key/nonce reuse after a lost reply | persist before send, no reseal, restore test | storage write quality |
| Removed member reads later | Remove + path + deletion | until next honest path commit |
| Compromised device | timed Update | window = update interval |
| Insider adds a device it does not own | certificate check on Add | devices the account signed |
| Insider fragments the group | MLS validation + epoch rule | detected, not prevented |
| Attachment swap | content addressing + per-file key | — |
| Oversized/malformed input | byte caps + fuzz | — |
| Metadata | buckets, documented residuals | mixnet deferred |
| openmls bug | exact pin, KATs, interop, review before release | **unaudited** |

## Test plan

1. Code-point pin (`0x004E`). 2. X-Wing cross-implementation KAT (crypto-pq vs
openmls libcrux) — if revisions differ, stop and record. 3. ML-DSA-65 ACVP
vectors through both implementations. 4. Device certificate accept / refuse
cases (wrong account, domain, expiry, altered field, missing signature).
5. Commit with a bad co-signature refused. 6. Lifecycle: 3 members × 2 devices,
add/message/remove/update. 7. PCS: old snapshot cannot read after an honest
Update. 8. Deletion: old secret-tree keys gone. 9. MLS generation rewind
regression (mirror of [ADR-074](ADR-074-chat-outbox-receipts-envelope-v2.md) test 1). 10. Byte-identical retry. 11. Two Commits
for one epoch: second refused; gap → resync. 12. Interop with another MLS
implementation if the harness supports `0x004E` (otherwise NOT VERIFIED).
13. Committed KAT vectors under `apps/chat/tests/vectors/mls/`.
14. Attachment tamper/order/key/cap/padding. 15. Fan-out to three devices.
16. State migration v1/v2 → v3. 17. Fuzz targets for group payload, device
certificate, chunk parser. 18. Measured sizes for Commit (with/without
co-signature), 1 KiB message, 120 KiB chunk — into `reports/`; no figure stated here.

## Build plan

0. Gate: Eric decides D2b; confirm pin and `0x004E`. 1. Pin + supply chain +
test 1. 2. X-Wing interop (test 2). 3. Device certificate (`group/device.rs`,
`group/validate.rs`, test 4). 4. Hybrid co-signature (`group/hybrid.rs` via
`crates/crypto-pq` `HybridMlDsa65SlhDsa128s`; tests 3, 5). 5. Encrypted MLS
storage (`group/store.rs`; tests 8, 9). 6. Group core (`group/mod.rs`,
`group/welcome.rs`; tests 6, 7, 13). 7. Relay and wire (`relay.rs`, `wire.rs`,
`net.rs`; tests 10, 11). 8. Fan-out and outbox (`courier.rs`, `outbox.rs`;
test 15). 9. Attachments (`attachments.rs`, blob endpoints; test 14, caps from
test 18). 10. Group receipts (after D2b). 11. Migration + fuzz (tests 16, 17).
12. Interop and vectors (tests 12, 13). 13. Bookkeeping in each user-visible PR:
features.toml (RESEARCH, unaudited), architecture-vision, CATALOG, site pages,
claims-check, report.

## Consequences

Groups can ship with a hybrid key exchange but not a hybrid application
signature until D2b. A draft-pinned library means a code-point change is an
ADR amendment plus a state migration. Fan-out cost grows with devices; group
cap and buckets are the levers. The co-signature adds ~11 KB per Commit only.
The blob store is new relay state with its own quota and review. Membership
size is visible to the relay. No "post-quantum group chat" claim before D2b and
an external review.

## Revisit when

D2b is decided or a composite suite lands upstream; the PQ MLS draft becomes an
RFC or renumbers; openmls has a stable release or a published audit; measured
fan-out exceeds budget; chat identity moves to `0x30`; chain-side account
recovery is decided.

## Not verified

openmls 0.9.2 tarball (feature names, `0x004E` mapping, wire-format policy);
draft revision match with `x-wing = 0.1.1`; openmls audit status and
`fork-resolution` semantics; interop harness support for `0x004E`. Sizes are
from FIPS 204/205 tables, not group measurements.
