#!/usr/bin/env python3
"""Z3 proofs over bit-precise models of two fee-market functions.

Master Prompt 8 §3 asks for "Z3 symbolic execution on small critical
functions". These are SMT proofs over bounded-integer models (every variable constrained
to the u64 range, the u128 intermediates checked for overflow) that follow
`crates/fee-market/src/split.rs::split` and
`crates/fee-market/src/base_fee.rs::next_base_fee` line by line, including the
u128 widening and the u64 saturation. Bounded integers rather than 64/128-bit
bit-vectors: the bit-vector encoding of a 128-bit multiply-then-divide did not
finish in 15 minutes; the integer encoding with explicit ranges proves the
same statements for the same domain. A proof here means: for *every* input in
the stated domain, the property holds -- not a sample.

The models are hand-transcribed, so they share the Lean model's caveat
(formal/README.md): they are tied to the Rust by review and, for `split`, by
`crates/fee-market/tests/lean_differential.rs`, not by extraction.

Run: python3 formal/z3/fee_market.py   (exit status 0 iff every proof holds)
"""

import sys

from z3 import And, If, Implies, Int, Ints, Not, Solver, sat, unknown, unsat

BPS = 10_000
U64_MAX = (1 << 64) - 1
U128_MAX = (1 << 128) - 1
TIMEOUT_MS = 120_000


def prove(name, claim, *assumptions):
    """PROVED only on unsat of the negation; a timeout is reported as UNKNOWN,
    never as a pass."""
    s = Solver()
    s.set("timeout", TIMEOUT_MS)
    s.add(*assumptions)
    s.add(Not(claim))
    r = s.check()
    if r == unsat:
        print(f"PROVED   {name}")
        return "proved"
    if r == sat:
        print(f"FAILED   {name}\n  counterexample: {s.model()}")
        return "failed"
    print(f"UNKNOWN  {name} (solver gave up after {TIMEOUT_MS // 1000} s)")
    return "unknown"


def u64(*xs):
    return And(*[And(x >= 0, x <= U64_MAX) for x in xs])


def split_model(base, bps):
    """split(base, tip, bps): treasury = base * min(bps, BPS) / BPS (in u128);
    burned = base - treasury (in u64)."""
    share = If(bps <= BPS, bps, BPS)
    product = base * share          # the u128 intermediate
    treasury = product / BPS        # integer division by a constant
    burned = base - treasury
    return burned, treasury, product


def next_base_fee_model(parent, size, target, denom, floor):
    """next_base_fee for validated target/denom (both > 0)."""
    gap = If(size >= target, size - target, target - size)
    delta = (parent * gap / target) / denom
    rising = size > target
    up = parent + If(delta > 1, delta, 1)
    down = If(parent >= delta, parent - delta, 0)
    nxt = If(rising, up, down)
    capped = If(nxt > U64_MAX, U64_MAX, nxt)
    return If(capped > floor, capped, floor), nxt, parent * gap


def main():
    results = []
    base, bps = Ints("base bps")
    burned, treasury, product = split_model(base, bps)
    dom = u64(base, bps)
    results.append(prove("split: the u128 product never overflows", product <= U128_MAX, dom))
    results.append(prove("split: treasury fits in u64 (the Rust `expect` never fires)", treasury <= U64_MAX, dom))
    results.append(prove("split: treasury <= base (burned never underflows)", treasury <= base, dom))
    results.append(prove("split: burned + treasury == base (100%)", burned + treasury == base, dom))
    results.append(prove("split: a share above 100% is clamped and creates no value",
                         Implies(bps > BPS, treasury == base), dom))

    parent, size, target, denom, floor = Ints("parent size target denom floor")
    validated = And(u64(parent, size, target, denom, floor),
                    target >= 64 * 1024, target <= 16 * 1024 * 1024,
                    denom >= 8, denom <= 1024, floor >= 1)
    nxt, raw, prod = next_base_fee_model(parent, size, target, denom, floor)
    results.append(prove("next_base_fee: the u128 product never overflows", prod <= U128_MAX, validated))
    results.append(prove("next_base_fee: never below the floor", nxt >= floor, validated))
    results.append(prove("next_base_fee: an under-target block never raises the fee above max(parent, floor)",
                         Implies(size <= target, nxt <= If(parent > floor, parent, floor)), validated))
    results.append(prove("next_base_fee: an over-target block never lowers the fee",
                         Implies(size > target, nxt >= parent), validated))
    bounded = And(validated, size <= 2 * target)
    results.append(prove("next_base_fee: one step rises by at most parent/denom + 1 (size <= 2x target)",
                         raw <= parent + parent / denom + 1, bounded))
    proved = results.count("proved")
    print(f"\n{proved}/{len(results)} proved, {results.count('unknown')} unknown, {results.count('failed')} failed")
    return 0 if results.count("failed") == 0 and proved == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
