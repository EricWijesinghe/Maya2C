# Lattice proof of work

**Status: research branch. No consensus path reaches any of this code.**

`maya-lattice-pow` carries the verification half of a lattice-based proof of
work; `crypto::lattice` in the node carries the derivation that feeds it.
Neither is wired to an activation height, and the reason is at the bottom of
this document rather than the top only because the construction has to be
described before its problem can be.

---

## The puzzle

A miner is handed a lattice and must find a short vector in it.

The lattice is a Goldstein–Mayer random `q`-ary lattice in Hermite normal form —
the family the [Darmstadt SVP challenges] are drawn from. For dimension `n` and
modulus `q` the basis rows are

```text
b_0 = ( q,   0, 0, …, 0 )
b_i = ( x_i, 0, …, 1, …, 0 )      1 <= i < n, the 1 in column i
```

so the whole lattice is named by `q` and `n - 1` coefficients, and
`det(L) = q`. A basis that would otherwise be `n²` integers on the wire is
`n - 1`.

The coefficients are derived from the block header through a keyed BLAKE3 XOF
with rejection sampling (`crypto::lattice::basis_for_seed`). **A miner who could
choose the lattice would choose one they had already reduced**, and mine every
block for free. That the lattice comes from the header and nowhere else is the
property the entire scheme rests on.

## Verification is a divisibility test

Writing `v = Σ c_i b_i` gives `v_i = c_i` for `i >= 1` and
`v_0 = c_0·q + Σ_{i>=1} c_i·x_i`. Eliminating `c`:

```text
v ∈ L   ⟺   v_0 ≡ Σ_{i=1}^{n-1} x_i · v_i   (mod q)
```

So the miner sends `v` alone. The coefficient vector is recoverable from it,
which means carrying it would be redundant bytes a miner could vary to grind.

Validation is then two passes over `n` integers — one congruence, one sum of
squares — in exact integer arithmetic. Finding a short `v` is exponential;
confirming one is `O(n)` additions. That asymmetry is what makes the scheme
checkable at all.

## Why validation never runs LLL or BKZ

Because it cannot. Lattice reduction runs on floating-point Gram–Schmidt —
fplll uses `double`, `long double`, or MPFR depending on precision — and
floating-point results are not reproducible across platforms, compilers, or
library versions. A chain whose validity rule re-ran reduction would fork on a
rounding difference.

It does not re-run anything. The validator checks a congruence and a sum of
squares, both exact, both bounded by `MAX_COORDINATE` so neither can overflow.
Reduction is entirely the miner's problem, and its non-determinism never reaches
consensus.

## Why the difficulty target is a norm and not a Hermite factor

The obvious knob is the Hermite factor `δ`, since that is how lattice hardness
is usually quoted. It is the wrong knob for a retargeting chain.

With `β` the BKZ blocksize:

```text
δ  ≈ ( (β / 2πe) · (πβ)^(1/β) )^( 1 / (2(β−1)) )
cost(BKZ-β) ≈ 2^(0.292β + o(β))     sieving, BDGL
            ≈ β^(β / 2e)             enumeration
```

`δ` moves in the third and fourth decimal place while cost moves exponentially,
and **`β` is an integer**. The retarget clamp in `consensus::difficulty` is
`[1/4, 4]` per window; against this curve that range maps to a sub-integer step
in `β`, so difficulty could express *no change at all* and then a jump of several
orders of magnitude. A chain cannot retarget on that.

So the consensus rule is `‖v‖² <= T²`, with `T` a real-valued threshold. It is
continuous, it is exact in integers, and it decides the same question. `δ` stays
a reporting quantity.

Squared on both sides so that nothing takes a square root: integer `sqrt` is one
more rounding rule for two implementations to disagree about.

## What is proved

`crates/lattice-pow/src/proofs.rs`, under `cargo kani -p maya-lattice-pow`:

| Harness | Bound |
|---|---|
| `params_accept_exactly_the_documented_range` | **unbounded** over `u16 × u32` |
| `the_norm_equals_its_oracle_inside_the_coordinate_bound` | dimension 4 |
| `a_coordinate_outside_the_bound_is_always_reported` | 1 coordinate |
| `every_basis_row_is_a_point_of_its_own_lattice` | dimension 4 |
| `a_basis_is_accepted_exactly_when_it_is_well_formed` | dimension 4 |
| `the_zero_vector_is_never_accepted` | dimension 4 |
| `acceptance_implies_every_rule_held` | dimension 4 |

The folds are uniform — one multiply-and-reduce per coordinate in `contains`,
one square-and-add in `norm_squared` — so the induction from step `n` to `n + 1`
carries no new case. The honest reading: the *step* is proved over its whole
input range, the *fold* up to four coordinates.

**These harnesses have not been executed.** Kani is not installed on the
development host, and `#[cfg(kani)]` means `cargo check` does not compile them
either. They are unverified in both senses until someone runs them.

---

## Why this is not a consensus rule

### Lattice reduction is not progress-free

This is the blocking problem, and no part of this branch addresses it.

Nakamoto consensus assumes mining is a memoryless process: each attempt is an
independent Bernoulli trial, so a miner nine minutes into a block is no closer
than one starting now. That assumption is what makes hashrate share equal reward
share, and what makes "cumulative work" mean "expected trials spent".

BKZ is monotone optimisation. Elapsed work strictly improves the basis. So:

- a large miner finishes reductions a small miner must abandon, and reward
  becomes **superlinear** in miner size;
- partial work is not discardable when the tip moves, so switching to a new
  block is penalised — which is an incentive to mine stale;
- `consensus::difficulty` compares branches by expected trials, a quantity that
  no longer corresponds to anything.

Bounded-attempt reformulations exist — reduce for at most `k` steps, then hash
the result — but they reintroduce hashing as the thing being measured, which
gives up the premise. Constructions with real worst-case hardness reductions
(Ball–Rosen–Sabin–Vasudevan and successors) are the serious literature here, and
they are not this.

### The work is not useful

"Proof of useful work" would need the vectors to be worth something to someone.
Short vectors in per-block pseudorandom lattices are worth nothing — they are
discarded the moment the block is accepted.

Making them useful means mining against pre-registered targets, such as the
Darmstadt challenges. Those are finite and precomputable, which destroys the
unpredictability the previous section depends on, and a curated registry of
targets is a trusted party. Per invariant 11, introducing the chain's only
trusted party is a decision somebody writes down; nobody has.

The name is therefore **lattice proof of work**, not PoUW.

### It would be a fork, not a replacement

Blocks 1..N were mined under ArgonBlake and then the DAG. That code stays for
historical validation forever — a chain cannot retire the rule its own history
was made under. Any activation would follow `DagConfig`
(`crates/node/src/crypto/dag/registry.rs`): a height, and a difficulty reset to the floor,
for the reason documented there — a target calibrated against one algorithm's
cost is meaningless against another's, and being too hard stops a chain in a way
retargeting cannot recover from.

[Darmstadt SVP challenges]: https://www.latticechallenge.org/svp-challenge/
