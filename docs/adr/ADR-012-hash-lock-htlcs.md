# ADR-012: Hash-lock HTLCs are live; lattice locks stay RESEARCH

**Status:** Accepted
**Date:** 2026-09-22

## Context

Master Prompt 2 §8 asks for:

1. **REAL:** hash-locks on SHA3-256 / BLAKE3 preimages, with the reason they
   are already quantum-safe documented.
2. **RESEARCH:** Module-LWE commitment locks.
3. Lock, claim, and refund by block height, plus a watcher that claims
   before the timeout.
4. `tests/htlc_lattice_tests.rs` covering a successful swap, a refund on
   timeout, and a rejected forged preimage.

What the tree had was item 2 alone, with item 3 built around it. The lattice
lock ran end to end — `htlc-lattice`, the node's `htlc_exec`, `htlc-watcher`
— but everything sat dark behind `HTLC_L_ACTIVATION_HEIGHT = u64::MAX`, and
no hash lock existed.

## Decision

### 1. One lock type, two families

`maya_htlc_lattice::Lock` is either `Hash { function, digest }` or
`Lattice(Commitment)`, and `Unlock` is either a `Preimage` or an `Opening`.
Both encode with a family tag. The lock record becomes version 2, and
version 1 is refused rather than migrated: no version-1 record exists on any
chain, because no HTLC ever executed. One record type, one refund path and
one watcher serve both families. The timelock rule, the escrow accounting in
the invariant guard, and invariant 7's no-op races are shared unchanged.

A preimage is exactly 32 bytes. A variable length is the classic HTLC
length attack, where the two chains disagree about what was hashed.

### 2. Why a 256-bit hash lock is already post-quantum

The best known quantum preimage attack is Grover's search. Against a 256-bit
digest it needs about 2^128 *sequential* hash evaluations, and it
parallelises badly: k machines buy only a √k speed-up. Nothing plans around
2^128. Collision search (Brassard–Høyer–Tapp, about 2^85 on paper and worse
once memory is counted) is irrelevant here, because the lock's creator fixes
the digest from a preimage it chose, so there is no pair to collide.

### 3. SHA-256 as well as SHA3-256 and BLAKE3

The brief names SHA3-256 and BLAKE3, but neither Bitcoin nor Ethereum can
check either one. Bitcoin script has `OP_SHA256`, and Ethereum has the
`sha256` precompile (its native `KECCAK256` is not SHA3-256). A hash lock
the other chain cannot match is not a swap, so `HashFunction::Sha256` is
included. The quantum argument in §2 applies to it identically. The known
answer for 32 zero bytes is pinned in `lock.rs`.

### 4. Activation: hash locks at height 0, lattice locks at `u64::MAX`

`BlockContext` carries two gates. `HASH_LOCK_ACTIVATION_HEIGHT = 0` makes
hash locks **REAL**. Every transition is gated by the family of the lock it
touches: a claim or refund reads the record first, then checks that lock's
family.

Height 0 reinterprets no history. Before this change every HTLC payload
failed at execution, so no block on any chain contains one. A node without
this change rejects a block carrying a hash lock, as any upgrade's older
nodes do.

The lattice lock keeps its open questions (`docs/htlc-lattice.md`, "Before
anyone picks an activation height"):

- claim verification cost against a full block;
- whether a counterparty chain runs the verifier.

Neither question applies to a hash lock: a claim costs one hash, and the
other chain needs only its native hash opcode.

### 5. BLAKE3 without breaking the Kani build

`htlc-lattice` stays free of C code so the nightly `cargo kani -p
maya-htlc-lattice` can compile it, and `blake3` compiles assembly. Its
`pure` feature would unify across the workspace and slow every node hash,
and a cargo *feature* that gated BLAKE3 would let two node builds disagree
about a claim's validity (ADR-002). So `blake3` is a
`[target.'cfg(not(kani))'.dependencies]` entry. Every real build has it; only
the proof build, where cargo-kani sets `--cfg kani`, does not. The timelock
proofs never touch a hash.

### 6. Claims and refunds gate on the record, so an unknown lock is a no-op

A claim or refund now reads the lock's record before any gate, because the
record is what names the family. With no record there is nothing to gate,
and the outcome is `UnknownLock`, a valid no-op, as invariant 7 requires of
any losing settlement. Before this change the lattice gate ran first and
voided the whole block. The change is consensus-visible and is pinned by
`a_claim_or_refund_on_an_unknown_lock_is_a_valid_no_op_in_production`.

Version-1 records are refused rather than migrated, on the premise stated in
§1. That premise is a claim about deployment history, and the code cannot
check it. If any persisted network did hold a version-1 record, the node
would fail closed on it rather than misread it.

## Consequences

- `htlc-watcher` holds a `SwapSecret` (a hash preimage or a lattice secret).
  Secret files carry a family prefix (`sha256:`, `sha3-256:`, `blake3:`,
  `lattice:`), and bare hex still reads as lattice.
- `htlc_get_lock` reports `lock_kind`, `hash_digest` (what another chain's
  script locks under) and `unlock`, which replaces `opening`. It is an API
  change on an endpoint that could only ever have returned "not found".
- `crates/node/tests/htlc_lattice_tests.rs` runs the hash-lock group in the
  node's own production context, with no activation override:
  - a SHA-256 swap across two chains through the real watchers;
  - lock and claim under each function;
  - refund at `T`, with a late correct preimage as a no-op;
  - forged preimages (wrong, off by one bit, the other family's unlock)
    leaving the lock open;
  - a lattice lock refused where a hash lock is accepted.
- `features.toml` splits the entry: hash-lock HTLCs are REAL/verified, and
  lattice HTLC-L stays RESEARCH/working.
