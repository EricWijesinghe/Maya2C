# ADR-155: Format room: every growth path has a byte for it before genesis

**Status:** Accepted (2026-11-24). Eric 2026-11-24: "ensure the maximum
ceilings that could someday be used by other networks are already patched;
numberwise or any way, take no chance", and the 2026-11-15 rule that every
limit must be liftable without a hard fork.
**Date:** 2026-11-24

## Context

A hard fork here means one of three things: a field missing from bytes that
are signed or hashed into consensus (the vertex digest, a certificate record,
a header, a state record, a transaction), with no version byte to tell a later
layout apart; a value fixed in the genesis id with no key to move it; or a
bit or tag space that runs out. A compiled per-block constant that a new
binary moves at an activation height is a scheduled upgrade, not a hard fork.

The pre-genesis review of the ceilings found six format gaps of the first
kind. Each is cheap to close while there is no chain to stay compatible with,
and each would need every node, wallet, miner and light client to change at
once afterwards.

## Decision

Each item below is a guarantee the chain makes from its genesis block. They
ship as separate pull requests, stacked where an open PR already owns the
code; the rule IDs are in `spec/`.

### 1. Double-signing is provable at any block size (NEW-1, CON-18)

The vertex digest no longer streams every batch item. It is

```text
blake3("maya2c/dag-bft/vertex/v3" ‖ epoch ‖ round ‖ author_u32 ‖ timestamp_ms
       ‖ parents_root ‖ batch_root)
parents_root = blake3_derive_key("maya2c dag-bft vertex parents v1", n ‖ parent*n)
batch_root   = blake3_derive_key("maya2c dag-bft vertex batch v1",   m ‖ blake3(tx)*m)
```

A proposal signature covers the header, so equivocation evidence is two signed
headers (`1 ‖ epoch ‖ round ‖ author_u32 ‖ timestamp_ms ‖ parents_root ‖
batch_root ‖ sig`), 8,293 bytes at most each, whatever the two vertices carried.
Before, evidence was two whole `Propose` frames: two full 4 MiB batches made a
report of 8,415,023 bytes, twice the 4 MiB transaction cap, so the offence could
not be filed at all (`format_room_tests`, red run). The engine also keeps
headers, not vertices, as its per-slot equivocation memory.

### 2. Certificates name their format and carry ids for any committee (C2, NEW-7, CON-19)

A certificate record starts with a version byte (1); any other is refused.
Every validator id on the DAG-BFT wire (sender, addressee, author, voter) is
four bytes, and the digest hashes the author as four bytes. The in-memory
index stays `u16` in this build, and an id above 65,535 is refused at decode,
so a committee wider than that later is a binary change that alters no
encoding and no digest. The gossip frame version is 2.

Not changed here: the block attestation frame (ADR-038) still carries a
two-byte validator index. Attestations are not stored on chain yet; the
evidence format CON-05 adds for them (open PRs #680 and #686) should use four.

### 3. Block bytes and transaction count move by vote, inside compiled ceilings (B1, NEW-4)

`BlockMaxBytes` and `BlockMaxTransactions` become `ParameterKey`s (tags 21
and 22) with the block-validity lead of ADR-082 amendment b. Their ranges end at
compiled ceilings that every transport of the same build is asserted at
compile time to carry: one 8 MiB gossip frame with a mebibyte of framing, one
24 MiB sync response (ceilings 7 MiB and 815,559 transactions, the byte ceiling
at nine bytes an entry). Every apply path judges a block by the values of the
state it extends, its parent's; selection and the DAG-BFT decode budget read
the value in force where the block is built. On arrival a block is checked
only against the compiled ceilings, which no vote moves: checking it against
the importing node's tip would let two honest nodes on either side of an
activation height disagree about one side-branch or late block (PR #702
review). Raising a ceiling is a release with larger or chunked frames: a
scheduled upgrade, not a format change. The decoder reserves room for at most
4,096 transactions before any has decoded, so a frame's claimed count, read
before any proof of work or signature, never sizes an allocation.

The contradictions the review found are removed. The epoch sync bound is
derived from the epoch length and the ceiling instead of a 4 GiB constant a
full epoch exceeds. The governed `vm.max_module_bytes` is now the value a
deployment is held to, read from the state at execution and at mempool
admission, and its range ends at the VM's compiled `MAX_MODULE_BYTES`
(512 KiB), which is also the decoder's bound. Every size a vote can set is
one each node decodes, the VM accepts, and one block's contract gas deploys
(50,000 + 450 per byte is 235,979,600 at 512 KiB, under the lowest
`vm.max_block_gas` a vote can set). The earlier 2 MiB range was read nowhere:
no module over 512 KiB could deploy under any vote, and 2 MiB is past what the
300M gas ceiling can pay for (about 666 KB). A wider range is a release that
raises the VM cap, the decoder and the gas ceiling together.

### 4. The header has a slot for roots it does not compute yet (C12, B5, B6)

One 32-byte extension root is reserved in the header, inside the bytes the
block id and the proof of work cover. It is all zeros, and a header with any
other value is refused, until an activation height defines it as the root of
a versioned extension record: the ADR-053 ordered-set root, the body chunk
root, the certificate root. The header is 177 bytes; it stays inside three
BLAKE3 blocks, so the GPU kernels' block count does not change, and no earlier
byte range moved (the root follows the version byte).

Chunked gossip and sync framing is not part of this set. Framing is
node-local, not consensus, and item 3 asserts that a block at the highest cap
a vote can set fits one gossip frame and one sync response, so no valid block
needs chunks; the release that raises the ceiling past one frame brings them.

### 5. Upgrades name their binary and have room for any number of features (G21, NEW-3)

The upgrade record carries the 32-byte hash of the release that implements
it, and the feature set is 128 bits wide instead of 64. Bits keep their
numbers. The stored `up:v` record starts with a layout byte (1), so a set
wider than 128 bits later is a new layout of the same record, not a new
record kind. The hash is committed (proposal, record, genesis id) for
operators and voters to check a release against; no node compares it with
its own executable, which cannot hash itself reliably. A sunset-only entry
names no release. 128 rather than 256: bits 0-10 and the v4 claim 40-47 are
the whole demand in sight, and the layout byte covers the rest.

### 6. Accounts, transactions and proofs have their version bytes (P7, B17, P3)

- An account record is 41 bytes: balance, nonce, then an auth kind byte
  (0 = the address's own key, the only kind executed). The byte is in every
  state leaf (ROOT-1). A smart-account policy or another key scheme is a new
  kind a scheduled upgrade defines, not a new record layout. A record naming
  an unknown kind is refused.
- Transaction wire version 14 is reserved for a transaction carrying an
  access list, its declared read and write set (B17, pre-genesis half; the
  next number after the v4 umbrella's claims). It is refused at decode by name
  until the rule that defines its layout activates, so the number can never
  mean anything else. A field in versions 5 to 8 was the alternative: it would
  have changed every wallet, SDK and hardware-wallet encoder for a list that
  is empty until then.
- A STARK proof starts with a version byte (1). A later circuit, field or hash
  is a new version a verifier can tell apart from the bytes alone; an unknown
  one is refused before decoding.

## Consequences

- Every block id, state root, genesis id and certificate encoding changes.
  This is a re-genesis change, landing with the other re-freeze format fixes;
  the running testnet re-genesises.
- Equivocation evidence is smaller (two headers instead of two frames) and its
  size no longer depends on the block cap or the committee size.
- Validators widen nothing in memory today; the four-byte ids cost two bytes
  per id on the wire.
- Each refused value (an unknown certificate version, an id over 65,535, an
  unknown auth kind, an unknown proof version, a non-zero extension root) is a
  decode error, so an old binary meeting a new encoding stops instead of
  reading it as something else.

## Alternatives considered

- **Exempt evidence from the transaction cap** with its own bound tied to the
  batch cap. Rejected: the bound would have to grow with every raise of the
  block or batch cap, and a report of tens of megabytes is a cost on every node
  for a fact that fits in two headers.
- **Reserve bit 63 of the feature set as "extended set follows".** Rejected:
  two encodings of one set, where one width covers it.
- **A separate auth-kind record under its own prefix** instead of a byte in
  the account. Rejected: a second read on every signature check, and a state
  proof of an account would not prove how it authenticates.

## Invariants touched

- 24 (execution reproduces the declared root): roots change, the rule does not.
- 25 (state under the root): the auth kind is inside the account leaf.
- 31 (transaction wire): version 14 is reserved; versions 5 to 8 are unchanged.
- 33 (block byte cap): the cap becomes a governed value under a compiled
  ceiling; `block_bytes_tests.rs` pins both.
- 34 (upgrade schedule): the upgrade record layout widens.

## Verification

Red runs before each change and green runs after are in the pull requests and
`reports/sessions/2026-11-24-format-room.md`. Vectors: CON-18, CON-19 in
`spec/tests/consensus.json`; ROOT-1 in `spec/tests/state_transitions.json`,
regenerated by `cargo run -p maya-spec-ref --bin spec-vectors`.

## Prior art

Not a novelty claim; similar designs exist. Bitcoin's segregated witness
moved signatures out of the transaction id and Narwhal certifies batch
digests rather than batches; Ethereum's EIP-2930 access lists and Cosmos SDK
`Any`-typed account public keys are the patterns for items 6's first two
bullets. No dated search was run (Standing Order 10): this ADR makes no
"first" or "only" claim.
