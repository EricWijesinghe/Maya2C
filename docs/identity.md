# Self-Sovereign Identity

Who a subject is, which key speaks for them, and what an issuer has attested —
without the chain ever learning a fact about a person.

- `crates/identity/` — the records. Chain-free, so the decoder fuzzes alone.
- `crates/node/src/state/identity.rs` — prefixes, and their place in the state root.
- `crates/node/src/state/identity_exec.rs` — registration, rotation, revocation, anchoring.
- `crates/node/src/core/identity_payload.rs` — the wire forms.
- `crates/zk-privacy/src/credential.rs` — the disclosure circuit.
- `crates/node/tests/identity_tests.rs`, `fuzz/fuzz_targets/did_decode.rs`.

---

## The one rule

**No claim preimage reaches chain state, ever.** Only commitments and roots.

A chain is permanent and public. A date of birth written to it once is written
forever — no migration, no governance vote and no fork reaches it, because every
archive node already has the block. So the types are shaped so that putting a
claim on chain is not a mistake anyone could make: there is no field for one, in
any record.

`no_committed_record_holds_a_claim_preimage` **scans** committed state for the
bytes of a claim after a full issuance cycle, rather than reading the types.
"There is no field for it" is an argument about today's code; a scan is an
argument about the bytes.

---

## `did:maya2c:<address>`

The brief asked for `did:maya2c:<pubkey>`. A Maya2C public key is a hybrid pair
at **1,984 bytes** — about 2,712 base58 characters. A DID goes in a URL, a QR
code and every log line mentioning the subject; 2,712 characters is not an
identifier, it is a payload.

The chain already had the right value. An address is BLAKE3 over **both** public
keys, and `Transaction::sender` explains why that matters: hashing both forces
an attacker who breaks one scheme to also hold the victim's key in the other,
because a forged half paired with a self-chosen partner hashes to a different
address and owns nothing.

So a DID *is* an address — 44 base58 characters — and the binding argument is
one the chain already makes. Keys live in the document's verification methods,
which is what `did:ion` and `did:key` do for the same reason.

Base58 rather than hex (64 chars) or base64 (`+`, `/`, `=` all need URL
escaping). The leading-zero rule is load-bearing: without it `[0, 1]` and `[1]`
encode identically and two addresses share a DID.

---

## Rotation and revocation need no new cryptography

The brief asked for "post-quantum lattice proofs". ML-DSA-65 **is** a lattice
signature and every transaction carries one.

A rotation transaction sent from the subject's own address is, by construction,
signed by the key the document currently names — the address is the hash of that
pair. So the old key authorises the new one, and the proof is the signature that
already had to be there. A second proof system alongside would be another thing
to get wrong for nothing.

Three details that are decisions:

- **The subject is the sender, never a field.** There is no way to express "do
  this to somebody else's DID", so there is no authorisation check to get wrong.
  A `subject` field plus a check that it equals the sender is one refactor away
  from a `subject` field plus a check somebody removed.
- **The epoch advances by exactly one.** Not "at least one": a gap is a rotation
  nobody can point at, and standing still would let a replayed rotation
  reinstall an old key.
- **Revocation is a state, not a deletion.** A deleted document is
  indistinguishable from one that never existed, and the two mean opposite
  things: a subject that lost its keys, versus a string somebody made up.

---

## What is post-quantum here, and what is not

| | |
|---|---|
| Subject and issuer keys | ML-DSA-65 + SLH-DSA — post-quantum |
| An issuer's attestation | signed with that pair, verified natively by consensus — post-quantum |
| A holder's disclosure proof | Groth16 over BLS12-381 — **not** post-quantum |

The split is forced, not chosen. Selective disclosure normally verifies the
issuer's signature *inside* the circuit. Verifying ML-DSA-65 in R1CS means an
NTT over a 23-bit prime, 256-coefficient polynomials, a 6×5 matrix and SHAKE256
— on the order of **10⁷–10⁸ constraints**, against a joinsplit circuit that is a
few tens of thousands. Groth16 setup at that size is not something anyone ships.

BBS+ is the industry answer to exactly this problem and is pairing-based, so it
is not post-quantum either and defeats the premise outright.

So the two concerns are split. The issuer anchors a root in an ordinary
transaction; consensus verifies the hybrid signature **natively** and the root
becomes consensus state. The circuit then only proves membership under a root
the chain already vouches for — no signature in it at all.

A quantum adversary could forge a *presentation*, but not an *attestation*. Same
standing caveat as the shipped shielded pool, written down rather than left to
be inferred from a dependency list.

---

## The disclosure circuit

Three statements, conjoined:

> I know a value `v` and a blinding factor `r` such that `H(subject, schema, v,
> r)` is a leaf under `issuer_root`; **and** `v` satisfies the public predicate;
> **and** the revocation bit at my index is zero under `revocation_root`.

Public: two roots, a predicate tag and a bound — four inputs, pinned by a test.
Private: the subject, the value, the blinding factor, the index and both paths.

### Why the revocation bit is in the circuit

Because leaving it out would undo the rest. A holder who proves `age ≥ 18` in
zero knowledge and is then asked for credential index 4,721 has linked the
presentation to a credential — and across two verifiers, to itself.

The chain still publishes the bitmap, and reading one bit is the cheap path when
the index is not sensitive. This is the path for when it is. Same state, two
ways to read it, and the verifier chooses.

The revocation leaf is built with the bit **hard-coded to zero** rather than
witnessed. A witnessed bit would let a prover claim zero for a leaf whose real
value is one; with the constant, the only leaf that authenticates is the
unrevoked one, and a revoked credential has no path that reaches the root.

And the two paths must share an index. Without that equality a holder proves
membership of credential A and non-revocation of credential B — a revoked
credential presenting cleanly.

### The blinding factor is not optional

Ages, country codes and accreditation flags all come from small sets. Without a
blinding factor, a verifier holding the tree recovers the value by recomputing a
few dozen digests. The commitment is `H(domain, subject, schema, value,
blinding)`, and the schema carries a length prefix so no byte can be moved
across the boundary between fields.

### Predicates are coarse on purpose

`AtLeast(18)`, `AtMost(n)`, `EqualTo(n)`, and nothing else. Each is a public
input, so each is a thing the verifier learns; a predicate language rich enough
to express a birth date would leak one through the public input even though the
value never leaves the circuit.

### A comparator bug worth recording

`at_least` walks bit vectors and takes the value at the **most significant**
differing bit. The first version walked from the top down and overwrote on each
difference, so the *last* write — the lowest differing bit — decided the answer.
`37 >= 38` came back true, because bit 0 differs and 37 has a one there. The
walk now runs upward so the last write is the highest difference.

It was caught by `every_predicate_shape_agrees_with_its_native_twin`, which
checks the circuit against `Predicate::holds` for boundary values on both sides.
A test that only tried `age ≥ 18` against age 34 would have passed.

---

## State

One prefix, `i:`, one `StateLayer::Identity`, one `RECORD_LAYERS` entry. Three
sub-prefixes under it — `i:did:`, `i:att:`, `i:rev:` — separate because they
change at different rates: a revocation is one bit an issuer may flip daily, an
attestation root changes on reissue. Folding them together would mean rewriting
and journalling the root record on every revocation.

The layer folds only when non-empty, so a chain where nobody has registered a
DID produces exactly the state root it would have produced before this subsystem
existed. That is invariant 11's shape, and it is why adding identity to a
running chain is not a fork.

The undo journal covers all of it for free: these go through `put_record` into
`overlay.records`, which is what `capture_undo` walks.

---

## Status

**RESEARCH.** Nothing in consensus depends on an identity record — no other
subsystem reads one, no fee or validity rule consults a DID, and a chain with no
registrations is byte-identical to one built before the subsystem existed.

Promoting it means: a decision about who may be an issuer (today anyone with a
DID can anchor a root, which is right for a permissionless design and wrong for
a regulated one), a resolver endpoint, and a post-quantum proof system if the
disclosure half is to carry the same claim the attestation half does.
