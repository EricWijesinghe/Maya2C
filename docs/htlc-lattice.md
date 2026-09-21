# Lattice HTLCs (HTLC-L)

**Status: RESEARCH** for the lattice lock this document describes:
`HTLC_L_ACTIVATION_HEIGHT = u64::MAX` (`crates/node/src/state/context.rs`).
Every lattice-lock transaction is refused before that height, so no network
runs it until somebody writes down a height and the reasons below have answers.

**Hash locks are REAL** and share everything below except the lock itself:
SHA3-256, BLAKE3 or SHA-256 over a 32-byte preimage, live from genesis
(`HASH_LOCK_ACTIVATION_HEIGHT = 0`). Why that is already post-quantum, and
why SHA-256 is among them, is ADR-012.

| Piece | Where |
|---|---|
| Commitments, openings, the timelock rule, the lock record | `htlc-lattice` (chain-free) |
| Wire forms `HtlcLock` / `HtlcClaim` / `HtlcRefund` (tags 41–43) | `crates/node/src/core/htlc_payload.rs` |
| State records `h:lk:<lock id>`, `StateLayer::Htlc` | `crates/node/src/state/htlc.rs` |
| Execution | `crates/node/src/state/htlc_exec.rs` |
| `htlc_get_lock` RPC | `crates/node/src/rpc/server.rs` |
| Counterparty watcher | `htlc-watcher` |
| End-to-end swaps, refunds, forgeries | `crates/node/tests/htlc_lattice_tests.rs` |

## What the brief assumed, and what is true

**"Module-LWE commitments instead of SHA-256 pre-images" because of quantum
pre-image attacks.** SHA-256 preimages are not a quantum target anyone plans
around: Grover's algorithm needs about 2^128 *sequential* evaluations, and a
SHA3-256 hashlock is already post-quantum. The honest case for HTLC-L is
uniformity. A claim here rests on Module-LWE at ML-DSA-65's parameters — the
assumption every signature on this chain already rests on — rather than on a
second one. The "quantum pre-image attack" tests cannot simulate a quantum
adversary; what they do is attack each check with a forgery that is consistent
with every other check.

**"Listens for external chain secret revelations."** An atomic swap needs both
chains to check *the same* predicate. Bitcoin script cannot verify a Module-LWE
opening; a classical hashlock on one leg and a lattice lock on the other are
two unrelated secrets, and revealing one reveals nothing about the other. So
the counterparty chain is another chain running this verifier — in-tree, a
second Maya2C. `htlc-watcher`'s `SwapChain` trait is where another such chain
would plug in.

**"A valid lattice noise vector matching target commitment bounds."** A noise
vector alone is forgeable: for *any* `s`, `e = t − A·s` satisfies the equation.
An opening is the pair `(s, e)`, and what makes it hard to find is that every
coefficient of both is in `[−η, η]`. `crates/htlc-lattice/tests/lattice_tests.rs`
builds exactly that forgery, asserts the equation holds, and asserts the bound
refuses it.

## Construction

- Ring `R_q = Z_q[X]/(X^256 + 1)`, `q = 8,380,417`. Module ranks `K = 6`,
  `L = 5`, bound `η = 4`. These are ML-DSA-65's; `t = A·s + e` is its public key
  before the low bits are dropped. Finding an opening is Module-LWE search;
  finding a second is Module-SIS at `2η`.
- `A` is expanded from a 32-byte seed with SHAKE128 rejection sampling in the
  coefficient domain. Uniform in either domain, so the argument is unchanged;
  the matrices are not bit-compatible with FIPS 204 `ExpandA`, and nothing
  needs them to be.
- Multiplication is schoolbook over `i64` accumulators, reduced once. No NTT:
  `s` is short, so there is no modular multiplication at all, and a second
  hand-written transform on a consensus path is a liability rather than a
  speed-up until measurement says otherwise.
- A secret is 32 bytes of entropy, expanded by two SHAKE256 streams into the
  seed and the opening. `ZeroizeOnDrop`, filled in place by
  `LatticeSecret::generate(getrandom::fill)`.

| Encoding | Bytes |
|---|---|
| Commitment (seed + `t` at 23 bits) | 4,448 |
| Opening (one nibble per coefficient) | 1,408 |
| Hybrid signature on the same transaction, for scale | 11,165 |

### Refused at decode, before any state is read

- A `t` coefficient `≥ q` — a second encoding and a second commitment id.
- An opening nibble past `2η` — a second encoding of a claim.
- A commitment every coefficient of which is already within `±η`: `s = 0,
  e = t` opens it, so anyone could claim.

## Consensus rules

| Rule | Why |
|---|---|
| A claim executes iff `height < expiry_height` and the opening verifies; a refund iff `height ≥ expiry_height` | The windows partition every height; Kani proves no lock admits both and no open lock admits neither (`crates/htlc-lattice/src/proofs.rs`). Heights, never timestamps — invariant 9 |
| A claim or refund that loses is a no-op | They race at the boundary as the ordinary case. An `Err` would let the loser — or anyone with a wrong opening — void the winner's block. Invariant 7 |
| A lock that cannot be made is an error | It is the sender's own transaction; no counterparty's block is at stake |
| The recipient is fixed in the lock | A front-runner copying an opening can only pay the right party early |
| Claims and refunds are permissionless | So a watcher can settle for its owner |
| The claimed record keeps the opening | Block bodies are prunable (invariant 27); the counterparty's watcher and light clients still need it, under the root |
| `h:` is a `RECORD_LAYERS` prefix, folded as `StateLayer::Htlc` | Invariant 25; the undo journal covers it through `put_record` (invariant 8) |
| Escrow is a conservation term | `fold_htlc_lock`: a lock moves value out of `acct:` into `h:lk:`, a settlement moves it back |
| **Only locks are breaker-gated** | `Module::of(HtlcLock) = Some(Htlc)`; claims and refunds map to `None`. A breaker that halted claims while an expiry passed would let the refund through and hand the swap to the refunder |

## The swap, and what the watcher guarantees

Alice holds the secret and has coin on chain A; Bob has coin on chain B.

1. Alice locks on A for Bob, expiry `T_A`, under commitment `C`.
2. Bob's watcher checks Alice's lock (recipient, amount, `C`'s id), checks the
   pairing below, and locks on B for Alice, expiry `T_B`, under the same `C`.
3. Alice claims on B. The opening is now in B's state.
4. Bob's watcher reads it and claims on A.

**Bob claims as soon as he sees an opening, without waiting for confirmations.**
The opening's validity is arithmetic; a reorg on B cannot make it stop opening
`C`. Confirmations matter only for deciding a swap is finished.

**Pairing.** Bob locks only if, measured from both tips at the same moment,

```text
tip_A + ceil((T_B − tip_B + conf_B) × rate) + submission_blocks + conf_A < T_A
```

where `rate` is a configured *upper bound* on A-blocks per B-block. A reveal at
B's last admissible height is then visible, and Bob's claim included and
confirmed, before A's expiry. Heights on two chains do not convert exactly; the
rate is conservative by construction and the watcher refuses rather than
estimates.

**Alice never reveals late.** Her watcher stops broadcasting a claim once
`tip_B + 1 + submission_blocks + conf_B ≥ T_B`. A claim that lands after `T_B`
does nothing on B — but its opening is still published, so Bob refunds B *and*
claims A. Revealing late is the one way the secret holder loses both legs.

**One secret, one swap.** A published opening claims every lock under its
commitment on every chain. The watcher refuses a commitment id it has already
journalled.

## Before anyone picks an activation height

1. **Verification cost.** `cargo bench -p maya-htlc-lattice`, measured
   2026-09-13 with a build linking on the same machine, so an upper bound:

   | Benchmark | Time |
   |---|---|
   | `expand_matrix` | 103 µs |
   | `decode_and_verify_claim` | 0.72–0.83 ms |
   | hybrid signature verification, for scale | ~0.18 ms |

   A claim transaction is about 12.7 KB (opening plus hybrid signature), so an
   8 MiB block holds about 660 of them: roughly half a second of verification
   on top of the signatures. Bytes bound it today. Whether that bound is enough
   — or claims need a per-block cap read from the parameter table (invariant
   17) — is the first thing to decide.
2. **A counterparty.** Without a second chain running this verifier there is
   nothing to be atomic with.
3. **State proofs for Identity and RWA.** `LAYER_ORDER` and
   `StateLayer::from_tag` in `crates/node/src/state/proof.rs` omit `Identity` (9) and `Rwa`
   (10), so an `AccountProof` from a state holding either fails to decode or
   verify. HTLC (11) is in both; a light client proving a revelation from a
   state that also holds identity or RWA records would hit the pre-existing gap.
