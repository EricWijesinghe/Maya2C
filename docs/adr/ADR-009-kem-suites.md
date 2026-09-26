# ADR-009: KEM suites and the two combiners

**Status:** Accepted
**Date:** 2026-09-21

## Context

`maya-crypto-pq` already ships ML-KEM-768 (`kem`) and HQC-192 (`hqc`), used by
custody sealing and the P2P handshake. The brief asks for ML-KEM-768 and -1024,
HQC-128 and -256, a dual-KEM combiner where breaking one KEM is not enough, and
an X-Wing-style hybrid with X25519. Transport wiring is Master Prompt 7; this
ADR only fixes the primitives.

## Decision

| Id | KEM | Standard | Backend |
|---|---|---|---|
| `0x01` | ML-KEM-768 | FIPS 203 (final) | `ml-kem =0.3.2` |
| `0x02` | ML-KEM-1024 | FIPS 203 (final) | `ml-kem =0.3.2` |
| `0x11` | HQC-128 | **draft** (NIST 2025 selection, FIPS 207 not final) | `hqc-kem =0.1.0-rc.0` |
| `0x13` | HQC-256 | **draft** | `hqc-kem =0.1.0-rc.0` |
| `0x21` | X-Wing (ML-KEM-768 + X25519) | draft-connolly-cfrg-xwing-kem | `x-wing =0.1.0` |
| `0x31` | DualKem ML-KEM-768 + HQC-128 | this ADR | combiner below |
| `0x32` | DualKem ML-KEM-1024 + HQC-256 | this ADR | combiner below |

HQC-192 stays where it is (the existing handshake); it gets no new id.

**DualKem combiner.**

```
ss = SHA3-256( "maya2c.dualkem.v1" ‖ id
             ‖ ss_mlkem ‖ ss_hqc
             ‖ ct_mlkem ‖ ct_hqc
             ‖ ek_mlkem ‖ ek_hqc )
```

Binding both ciphertexts is what the brief asked for. Binding the encapsulation
keys too is the X-Wing lesson: without it, a KEM that is not
ciphertext-binding lets an attacker re-target one component's ciphertext to
another key. SHA3-256 is used rather than BLAKE3 because it is the hash
FIPS 203 itself uses and the one X-Wing's proof is written for.

**X-Wing** is used exactly as the draft specifies, through the RustCrypto
crate; its test vectors are run from our tests, not only the crate's.

Every HQC type, log line and report row carries the word *draft*.

## Alternatives considered

- **Concatenate-then-HKDF without the keys.** Cheaper, and secure when both
  KEMs are IND-CCA and ciphertext-binding; ML-KEM is, HQC's binding has not
  had the same scrutiny. Four more hash inputs is the cheaper risk.
- **Classic McEliece instead of HQC.** More conservative, but 261 KB public
  keys for category 1 would not fit a handshake.

## Consequences

- A DualKem handshake costs both KEMs: 1,184 + 2,241 bytes of key and
  1,088 + 4,433 bytes of ciphertext at the lower level. Figures in
  `reports/02-crypto.md`.
- HQC's pin is on a release candidate of a draft; a bump is a transport
  compatibility review.

## Addendum 2026-09-21 — HQC decapsulation is not constant-time

`benches/dudect.rs` (inputs prepared before measurement, null controls
alongside) measured `hqc-kem =0.1.0-rc.0` HQC-128 decapsulation of a valid
ciphertext against a random one at **|t| = 26.9** (null control, valid vs
valid: 2.5). ML-KEM-768 (|t| = 2.2) and X-Wing (1.4) show no leak at the same
sample sizes. Raw output: `reports/dudect.txt`.

Consequences, until an HQC implementation measures clean:

- HQC is behind `maya-crypto-pq`'s `hqc` feature, **off by default**, so no
  crate uses it without asking. The one crate that asks is the node, for the
  opt-in `network::pq::dual` handshake (`--dual-kem`, default off), and that
  use is safe *because it is ephemeral*: a timing side channel on
  decapsulation recovers a key only by accumulating many chosen-ciphertext
  decapsulations under the same key, and the handshake draws a fresh HQC key
  pair per connection, so every key decapsulates exactly one ciphertext. HQC
  must never be used with a static decapsulation key (custody sealing, any
  long-lived identity) until it measures clean.
- A dual KEM whose HQC half leaks is, against a timing adversary, only as
  strong as its ML-KEM half. The combiner is still correct; it is the
  "breaking one family is not enough" claim that weakens to "breaking ML-KEM
  is still necessary".
- Re-run `cargo bench -p maya-crypto-pq --bench dudect -- --filter hqc` on
  every `hqc-kem` bump; the pin exists partly so that this result stays true
  of the code that ships.

## Addendum 2026-09-27 -- the figure was partly the harness; the leak is not

The 26.9 above came from a harness whose two classes allocated their
ciphertext buffers through different paths (one from `encapsulate`, one from a
fresh `Vec`), which the timer can see. `benches/dudect.rs` now allocates both
alike. On an idle machine (`scripts/dudect.sh`):

| Run | n | max abs t | tau |
|---|---|---|---|
| Fixed size | 2k | 10.3 | 0.25 |
| Continuous, 5 minutes | 176k | **47.5** | 0.11 |
| Null control (valid vs valid) | 25k | 2.3 | 0.01 |

A t that keeps growing with n at a steady tau is a leak, not noise, so the
consequences above stand unchanged. Inspection of `hqc-kem 0.1.0-rc.0`'s
decapsulation did not find the variable-time step (the comparison, the
decoders and the field arithmetic are all masked or fixed-bound); locating it
needs probes inside a vendored copy. Revisit when a new `hqc-kem` release, or
another HQC implementation, is available to measure with the same harness.
