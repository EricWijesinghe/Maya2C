# ADR-143: Signatures name their wire version and suite; headers reserve a hash id

**Status:** Accepted (2026-11-15). Eric 2026-11-14 MCQ, "all format fixes".
A transaction-validity and header-validity rule, taken before genesis.
**Date:** 2026-11-15

## Context

Two format gaps were cheap to close before genesis and expensive after it.

**What a signature commits to.** The signed message was
`domain ‖ chain_tag ‖ body`. The wire version was not in it, and for
versions 5 and 6 neither was the suite: the hybrid `0x30` was implied by the
version. Nothing replays today, and this ADR says so plainly: v5 and v6 share
a domain but differ by the payload tag (a v6 frame always carries one), and
v7 and v8 have their own domains. What was missing is the rule that makes
that hold by construction. A later version that reuses a domain — the v13
sponsored frame ADR-135 reserves is the obvious candidate — would have had to
get the separation right by layout alone. And the one pair where only the
version distinguishes two readings already exists: a hybrid key signs a v5
transfer, and the same key in a suite-`0x30` envelope (invariant 30) signs a
v7 one.

**What a migration off BLAKE3 would need.** `docs/CRYPTO_WATCH.md` §4 has a
rehearsed procedure (`crates/node/tests/hash_migration_rehearsal.rs`) for
re-committing state under a new hash at an epoch E, but no header said which
hash it was under. A light client could only learn E from somewhere else.

## Decision

### 1. The signed header (TX-6)

Every signed message is

```text
domain ‖ chain_tag ‖ wire_version:u8 ‖ suite:u8 ‖ body
```

| Version | Domain | Suite byte |
|---|---|---|
| 5, 6 | `custom-l1-node.tx.v5` | `0x30` |
| 7 | `custom-l1-node.tx.suite.v3` | the frame's suite |
| 8 | `custom-l1-node.tx.multisig.v3` | `0x00`: each key names its suite in the policy, which the body covers |

The version written is the one `to_bytes` writes, from one function, so the
encoder and the signer cannot disagree. The domains moved
(`tx.v4`, `tx.suite.v2`, `tx.multisig.v2` before) so that no message under the
old layout can be read under the new one, on any genesis. The txid is
unchanged: it already hashes a kind byte and the signatures.

### 2. The header hash id (CON-16)

The header's version byte (ADR-063, offset 144) is split:
`hash_id << 4 | layout`. `HashId` is a closed `#[repr(u8)]` enum with one
value, `Blake3 = 0`; any other id is refused at decode
(`unknown header hash id`), after the layout check, so the existing CON-10
vectors keep their errors. The hash id names the hash the header's block id,
proof-of-work seed, `tx_root` and `state_root` are computed under.

### Where a hash id is, and where it is not

The places are the ones a migration has to tell apart without consulting the
schedule. Everything else is computed at a height, by execution, and a
scheduled upgrade (ADR-082) already changes what execution does at a height.

| Commitment | Selector | Why that is enough |
|---|---|---|
| block id, PoW seed, `tx_root`, `state_root` | header hash id (new) | A header is read before its height is trusted. A light client must know the hash from the header's own bytes, and the CRYPTO_WATCH bridge block at E is the first header with a new id. |
| txid | wire version (now signed) | A new id hash is a new wire version activated by the schedule, like v7 was. Because the version is signed (§1), an old frame cannot be re-read as the new one. |
| sender address | suite (now signed) | `suite_address` is per suite. A new address hash is a new suite id, which invariant 29 already makes a code change plus a scheduled activation, and the agility migration window moves funds to it. |
| address text | Bech32m version symbol (ADR-081) | Already reserved; only 0 is defined. |
| stored ids (contracts, channels, pools, orders) | none | Names, derived once during execution at a height. They keep the hash they were made under; new ones follow the schedule. |

### Rejected

- **A 146th header byte.** `HEADER_LEN` = 145 is hard-coded in the XDP relay
  (`hal/ebpf-net`), the GPU and pool miners, Stratum V2, the light client,
  CON-1 and CON-12. The version byte was already strict (any non-zero value
  refused), so its high nibble costs no byte and changes no accepted header.
- **A hash id byte on every transaction frame.** It would duplicate the
  version, change every frame layout, and give a frame two fields that must
  agree. The signed version does the job.
- **A wider address.** `ADDRESS_LEN` = 32 is in every output, every state key
  and every wallet; the suite already selects the derivation.

## Consequences

- Every signature changes; every txid of a real signed transaction changes
  with it (the id hashes the signatures). Re-genesis is planned, so no
  persisted signature is lost. The Ledger parity fixture is regenerated and the
  device's review parses the header (`apps/ledger-maya2c/src/review.rs`).
- No header byte changes: every header valid before is valid now, and every
  one refused before is refused now; only the error for a high nibble changed.
- A hash migration remains RESEARCH (CRYPTO_WATCH §4). This ADR reserves the
  field; it implements no second hash.

Pinned by `crates/node/tests/signed_header_tests.rs`, `spec/tests/encoding.json`
(`signing_bytes_blake3`) and `spec/tests/consensus.json` (`header-hash-id-*`).
