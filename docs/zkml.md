# zkML inference verification

> **Retired 2026-09-21 (ADR-008).** `crates/zkml` and `crates/zkml-prover` were
> removed from the workspace: halo2 with KZG over BN254 is neither transparent
> nor post-quantum, and the SRS below was derivable from a public seed. The
> VM's `host_verify_zkml_proof` stays in the ABI and answers *no verifier*
> (`crates/node/src/state/zkml.rs`). zkML is PLANNED until it is re-expressed
> as a Plonky3 STARK over `crates/zk-stark`. What follows is the record of the
> removed system, kept because its measurements and its mutation sweep are
> the useful part.

A contract can now ask "did model *M* classify input *x* as class *c*?" and get
the answer from a proof, without running *M*. That works end to end, in a
block, and is measured. It is also dark on every network, not post-quantum, and
running on a setup anyone can forge proofs against. For every model it can
currently prove, verifying a proof costs 14,000–94,000× more than just running
the model. All four of those are true at once, and this document is where they are
written down together.

## What exists

| Piece | Where | Runs in |
|---|---|---|
| Model semantics, bounds | `crates/zkml/src/model.rs` | everywhere |
| halo2 circuit (KZG / BN254) | `crates/zkml/src/circuit.rs` | prover and verifier |
| Verifier | `crates/zkml/src/verify.rs` | **consensus** |
| SRS + mainnet guard | `crates/zkml/src/srs.rs` | consensus |
| ONNX importer, tract cross-check | `crates/zkml-prover/src/onnx.rs` | off-chain only (separate crate) |
| Key generation, proving | `crates/zkml-prover/src/prove.rs` | off-chain only (separate crate) |
| `host_verify_zkml_proof`, its price | `crates/vm/src/zkml.rs` | consensus |
| Node binding, activation, startup guard | `crates/node/src/state/zkml.rs`, `crates/node/src/state/context.rs` | consensus |
| Fixture generator | `scripts/make_zkml_fixture.py` | build time |

## Two crates, and why

| Crate | Contains | Rust | Who depends on it |
|---|---|---|---|
| `maya-zkml` (`crates/zkml/`) | circuit, verifier, SRS, model bounds | 1.88 | the node |
| `maya-zkml-prover` (`crates/zkml-prover/`) | ONNX import (tract), key generation, proving, the soundness and proof tests, the bench | 1.91 | the node's *tests* only |

This began as one crate with a `prover` feature. A dependency audit found
RUSTSEC-2026-0217 — an out-of-bounds read in the `tract-nnef` tensor parser —
in the tract 0.21.10 the resolver had chosen. Every patched 0.21 release pins a
`libm` that conflicts with the exact wasmtime the VM is pinned to, so the fix
is tract 0.23, which needs Rust 1.91. A package has one `rust-version`; raising
the verifier's would have raised the floor of everything that links the node,
including the SDK wheels CI builds on 1.88. So the prover became its own crate.
It also turned invariant 20 from a feature flag somebody must leave off into a
fact about the dependency graph.

tract 0.23 brought one cost with it: `dyn-eq`, the only copyleft crate in the
tree (MPL-2.0, file-level, used unmodified). `deny.toml` admits it by name, with
the reasoning, rather than admitting its license — so the next MPL crate fails
the check and has to be argued for on its own.

## Measured

| Quantity | Value | Source |
|---|---|---|
| Verification, release build, including parsing the key | **6.13 ms** (95% CI 5.99–6.25) | `cargo bench -p maya-zkml-prover --bench verify` |
| SRS derivation, once per process | 941 ms | same bench |
| Verifying key | 362 bytes | `proof_tests.rs` |
| Proof | 1,280 bytes | `proof_tests.rs` |
| Guest fuel rate, tight loop | 16.6 M fuel/ms | `crates/vm/tests/fuel_calibration_tests.rs` |
| Guest int8 matmul | **28.4 fuel per multiply-accumulate** | `crates/vm/tests/tensor_gas_tests.rs` |
| Verification price | 150 M fuel + 16/byte + 1,000/public input | `crates/vm/src/zkml.rs` |

The brief's budget was sub-10 ms. It is met, on this machine, with the key
parsed from bytes on every call — which is what the host function does.

The 941 ms is paid by the first verification in a process, which will be inside
a block. Warming it at startup belongs in whatever change sets an activation
height, not here: today nothing would ever call it.

## What the brief assumed, and what is actually true

| Brief | Reality | What was built |
|---|---|---|
| Load ONNX with tract *inside* the contract VM | A tract build is megabytes against a 512 KiB module cap with SIMD off; tract in the *host* is float inference inside consensus | tract is off-chain only, in `maya-zkml-prover`. It imports weights and cross-checks the circuit. The node depends on the verifier crate alone |
| Verify "without re-running inference" | Correct, and it also makes item 1 unnecessary | The verifier checks a proof; nothing in consensus runs a model |
| halo2 | Sub-10 ms needs KZG; IPA verification grows with the circuit | `halo2-axiom` 0.5.3, KZG on BN254, SHPLONK |
| Gas for tensor ops in the sandbox | Gas was a halting bound and **no host function charged anything** | Tensor ops stay in the guest, where fuel already prices them exactly. The one native operation, verification, is charged a measured price *before* it runs |
| ONNX → circuit (ezkl) | ezkl is not on crates.io: git forks tracked by branch, no license GitHub detects | A hand-written circuit for one model shape, which `cargo deny` accepts |

## The circuit

One arithmetic gate, `fa·a + fb·b + fm·a·b + fc = c`, and one lookup into
`[0, 512)`. Weights are fixed columns, so they are committed in the verifying
key, so **the key is the model** and the model id is `BLAKE3(domain ‖ key)`.
Key generation is deterministic: anyone holding the ONNX file can recompute a
contract's model id and check it.

It proves exactly this, and refuses any ONNX graph that is not exactly this:

```text
acc    = x · W1 + b1                    int8 × int8 → int32
hidden = min(max(acc, 0) >> s, 127)
logits = hidden · W2 + b2
class  = argmax(logits)                 first maximum wins a tie
```

Up to 16 inputs, 16 hidden units, 8 classes. The bounds keep every
intermediate under `2^20`, far below the 27-bit range checks — the margin is
what makes a "negative" field element (a number near `2^254`) impossible to
mistake for a small positive one.

### Soundness is tested, and the tests are tested

Every guard has a test that hands the circuit a witness which lies in exactly
that way. The lie is *consistent*, with everything downstream recomputed, so
the only constraint that can refuse it is the one the test is named after.

That second property is not decorative. The first version of these tests passed
with the ReLU sign check deleted, because a lie that was not propagated forward
got caught by some unrelated constraint first. So each guard was then deleted
in turn and the suite re-run:

| Deleted guard | Caught by |
|---|---|
| ReLU sign check | `claiming_relu_zeroed_a_positive_accumulator_is_refused` |
| ReLU bit booleanity | `a_relu_bit_of_two_is_refused` |
| quotient recomposition | `a_quotient_that_does_not_recompose_the_accumulator_is_refused` |
| remainder ≥ 0 | `a_negative_remainder_is_refused` |
| remainder < 2^s | `a_remainder_at_or_above_the_divisor_is_refused` |
| saturation booleanity | `a_saturation_flag_of_minus_one_is_refused` |
| saturation gap | both saturation tests |
| input ≥ −128 / ≤ 127 | `an_input_outside_int8_is_refused_even_with_a_consistent_witness` |
| one-hot count | `an_empty_one_hot_vector_is_refused` |
| one-hot index | `a_claimed_class_disconnected_from_the_one_hot_vector_is_refused` |
| one-hot booleanity | `a_non_boolean_one_hot_vector_is_refused` |
| argmax slack | `claiming_the_wrong_class_is_refused`, the tie test |
| **quotient range** | **nothing** — implied by the sign check, the remainder bounds and the saturation gap; the argument is at the constraint |

Five of those tests exist because the sweep found the gap. The circuit was
sound against all five exploits the whole time; nothing proved it.

## Three evaluators, no shared code

The fixture's expected outputs come from numpy. tract evaluates the ONNX graph.
`QuantizedMlp::evaluate` is what the circuit constrains. The tests require all
three to agree on every logit and every class. A test comparing the circuit to
itself would pass for any circuit.

## The economics, which decide whether any of this is worth using

Verification is priced at 150 M fuel because that is how long it takes. Running
a model in the guest costs 28.4 fuel per multiply-accumulate. So:

> **One verification costs the same as re-running a ~5.3 M-MAC model on-chain.**

The fixture classifier is 56 MACs. The largest model the circuit accepts is
16·16 + 16·8 = 384 MACs. Re-running the fixture in the guest costs ~1,600 fuel
against 150 M to verify it — **~94,000× cheaper**; for the largest provable
model, ~14,000×. For every model this module can prove today, re-execution
wins by four orders of magnitude or more.

zkML earns its cost in two situations, and this module serves neither yet:

1. **Models above the crossover** — millions of MACs, a small CNN. That needs
   convolutions, a much larger `K`, and almost certainly a circuit generated
   rather than hand-written.
2. **Private inputs.** Here the input is public: it is in the instance column,
   because a contract has to know what was classified. A private-input variant
   would commit to the input and expose the commitment instead.

What this module *is* is the foundation both of those need: a verifier that is
safe in consensus, a price that is measured, a host interface that cannot be
misused, and a circuit whose soundness has been checked by deletion.

## Security model

**Not post-quantum.** KZG on BN254 rests on pairings; a quantum adversary forges
proofs that any model said anything. This chain already carries one such
exception, the Groth16 shielded pool. This is a second, with the same kind of
guard.

**Not trusted, and worse than untrusted.** The SRS is derived from a seed in
`crates/zkml/src/srs.rs`. Its toxic waste is *public*: anyone who reads that file can
forge a proof for any model and any output today. That is the right setup for
tests and wrong for anything else. `SRS_IS_TRUSTED` is `false`.

**Dark.** `ZKML_ACTIVATION_HEIGHT` is `u64::MAX`. Every block the node builds
has zkML off; the import resolves, and calling it traps with
`ZkmlUnavailable`, identically on every node.

**Guarded.** `bins/maya2c-node/src/main.rs` calls `state::zkml::check_setup` at startup
beside the Groth16 check. It is inert while dark, and the moment a height is
chosen it refuses mainnet until the SRS is trusted — so choosing a height cannot
also quietly choose mainnet.

### Hostile-input handling in the verifier

- Every buffer is bounded before parsing (16 KiB key, 16 KiB proof, 17 inputs).
- The key's declared `k` is checked by hand first. halo2 would otherwise build a
  `2^k` evaluation domain from it; `k = 30` is a request for gigabytes.
- `SingleStrategy`, never `AccumulatorStrategy`, which draws from `OsRng`.
- The exact public vector is bound, length included: `VerifierSHPLONK` absorbs
  every public value into the transcript, so a second "class" appended after the
  real one — junk, or a zero that would leave the instance polynomial unchanged —
  fails. That is a property of the verifier *choice*, pinned by two tests,
  because a commitment-querying verifier would accept trailing zeros.
- Trailing bytes after a key or a proof are refused: a transaction id covers its
  bytes, so tolerated padding is a free way to mint distinct transactions
  carrying one proof.
- A panic inside halo2's parsers is caught and reported as a malformed key. A
  panic unwinding out of a host function would take the node down.
- Every key byte flipped and every truncation tried: never `Ok(true)`, never a
  panic.

### What the guest sees

| Outcome | Guest sees |
|---|---|
| proof verifies, key hashes to the named model | `1` |
| proof does not verify, or does not parse, or the key is another model | `0`, and the call continues |
| key malformed, buffer oversized, negative length, out of bounds | trap |
| not enough fuel | trap, **before** the verifier runs |
| zkML not active | trap (`ZkmlUnavailable`), never `0` |

A bad proof is `0` rather than a trap because a failing transaction fails its
whole block here, and a trap would hand anyone who can put a bad proof in a call
a way to void blocks. "Unavailable" is a trap rather than `0` because a contract
written for a live verifier must not quietly treat every proof as bad on a node
where the feature is off.

The model id is a *parameter*, not a result — like `oracle_read`'s staleness
bound. A contract cannot verify a proof without naming the model it expects.

## Gas

Before this, gas was purely a halting bound and no host function charged
anything. `host_verify_zkml_proof` is the first to charge for native work, and
it charges first: a call that cannot pay traps without a byte read or a pairing
computed.

150 M fuel is **fifteen times the VM's default gas ceiling**. A contract that
verifies must ask for a larger `gas_limit`, and should compute it with
`maya_vm::zkml::verification_fuel` rather than guess.

This does not fix the gap that predates it: **there is no cap on a call's
`gas_limit` and no per-block gas limit.** A fuel price bounds a verification
relative to a limit the sender chooses. That, with the SRS, is why this is dark.

## Before an activation height is chosen

- [ ] A real universal SRS (a published powers-of-tau ceremony converted to
      `ParamsKZG` at `K = 11`), its hash pinned in a test, `SRS_IS_TRUSTED`
      flipped by somebody who writes down why.
- [ ] A per-call `gas_limit` cap and a per-block gas limit.
- [ ] SRS warm-up at node startup, so the first verification in a block is not
      a 941 ms outlier on one validator.
- [ ] A model worth proving — above the ~5.3 M-MAC crossover, or with private
      inputs. Until then re-running the model in the guest is the right answer.
- [ ] `rust-reviewer` and `security-reviewer` passes on `crates/zkml/src/circuit.rs`
      and `crates/zkml/src/verify.rs` — the consensus- and crypto-critical parts.

## Reproducing the numbers

```powershell
cargo test -p maya-zkml-prover                             # circuit + proofs, 38
cargo test -p maya-vm --test zkml_host_tests --test tensor_gas_tests
cargo test --test zkml_block_tests                         # in a real block
cargo bench -p maya-zkml-prover --bench verify             # the 6 ms
cargo test -p maya-vm --release --test fuel_calibration_tests -- --nocapture
```

The fixture:

```powershell
uv venv .zkml-venv; uv pip install --python .zkml-venv onnx numpy
.zkml-venv/Scripts/python scripts/make_zkml_fixture.py
```

It is deterministic; regenerating it must reproduce the SHA-256 pinned in
`crates/zkml-prover/tests/proof_tests.rs`.
