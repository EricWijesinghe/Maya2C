# ADR-014: Threshold Raccoon for threshold lattice custody (RESEARCH)

**Status:** Accepted
**Date:** 2026-09-27
**Supersedes:** ADR-011's "no scheme chosen" for the `threshold-lattice` feature

## Context

Master Prompt 2 §7 asks for true threshold lattice signing (DKG plus partial
signature aggregation, the full key never in one place) as RESEARCH behind a
feature flag, *enabled only after a peer-reviewed scheme is chosen in an ADR*,
and a 3-of-5 test with simulated node failures. ADR-011 left the feature as an
interface that refused, because no scheme had been chosen.

## Decision

1. **The scheme is Threshold Raccoon (TRaccoon-128).** del Pino, Katsumata,
   Maller, Mouhartem, Prest, Saarinen, *Threshold Raccoon: Practical Threshold
   Signatures from Standard Lattice Assumptions*, **EUROCRYPT 2024** (ePrint
   2024/184). It is the peer-reviewed choice with the most to recommend it:
   `T`-of-`N` up to 1,024 signers in three rounds, security from Hint-MLWE and
   SelfTargetMSIS (the ML-DSA family of assumptions, no pairings, no trusted
   setup), and parameters that are concrete in the paper (Table 2, kappa = 128:
   n = 512, log q = 49, sigma_t = 2^20, sigma_w sqrt(T) = 2^42, nu_t = 37,
   nu_w = 40, l = 4, k = 5, omega = 19).
2. **The port follows the authors' reference, and is pinned to it.** The
   paper leaves implementation choices open (domain headers, MACs in place of
   round-two signatures, the modulus). `masksign/ec24-thrc` is the Raccoon
   team's own code for the paper; `custody-mpc::threshold` ports its
   `thrc-py` at commit `8ef114a`. `scripts/traccoon_vectors.py` runs that
   reference through a 3-of-5 session (parties 1 and 3 offline) with NIST's KAT
   DRBG and records a digest of every step; `threshold_tests.rs` reproduces
   `A`, the key, all five shares, all 25 pairwise seeds, every commitment,
   mask, MAC and response share, and the signature, **bit for bit**. The one
   input replayed rather than recomputed is the Gaussian noise: the reference
   samples at 256-bit mpmath precision, which no `f64` sampler reproduces
   integer-for-integer, so the recorded samples are fed in and the test checks
   that the port asks for each with the reference's seed and width. The
   production sampler (`threshold::gauss::PolarSampler`, the same polar method
   in `f64`) is checked for its distribution instead.
3. **Key generation is dealerless, and that part is not peer-reviewed.** The
   paper assumes a trusted dealer ("the design of a suitable DKG is outside of
   the scope of this work"); a dealer holds the whole key while it runs, which
   is what §7 forbids. `threshold::dkg` is this repository's construction: each
   party contributes a small `(s_p, e_p)` at `sigma_t^2 / N`, commits to
   `u_p = A s_p + e_p` before revealing it (so the last party cannot choose
   the key), and Shamir-deals `s_p`. The group secret `sum_p s_p` is never
   formed anywhere. `protocol::keygen_dealer` (the paper's algorithm) is kept
   only because the vectors come from it.
4. **It stays RESEARCH, off by default, and signs nothing on chain.** A
   TRaccoon signature is a *Raccoon* signature. Raccoon is not a NIST standard,
   so no suite in the registry verifies one and none will be added for it
   (invariant 29: only standardised parameter sets with NIST known answers
   enter a signature path). It is for a custody quorum's off-chain
   authorisation, verified by `threshold::protocol::verify`.
5. **The old `ThresholdScheme` trait is removed.** It required aggregated
   signatures to verify under a registry suite, which TRaccoon cannot meet
   (decision 4) and nothing implemented. Keeping an interface no scheme
   satisfies would be dead code describing a design that does not exist.

## Where the port departs from the reference, deliberately

Both reviews (rust-reviewer, security-reviewer) ran before commit; these are
the changes they drove, and each is a departure from `thrc-py`:

- **The signing nonce is bound to the session.** The reference derives
  `r_j` from the caller's key material alone, so a caller that reused a key
  across two sessions would publish two responses under one `r_j` with
  different challenges, and `z_j - z_j' = lambda_j (c - c') s_j` gives up the
  share exactly as a reused Schnorr nonce does. The public `sign_1` hashes the
  key with the session hash (over `sid`, `mu`, `act`) first. The reference's
  derivation survives only as crate-private `sign_1_reference`, driven by the
  known-answer test, which therefore lives in `src/threshold/kat.rs`.
- **The DKG enforces its own order.** `Dealer::reveal` and `private_share`
  refuse until `commitments_received` has every party's commitment (and the
  one in this party's slot is its own), so commit-before-reveal is a property
  of the type rather than of the caller.
- **Rosters are bounded:** `2 <= T <= N <= 1024`. The upper bound is the one
  the parameters are proven for and keeps party indexes far inside the 24-bit
  domain headers, which now assert rather than truncate. `T = 1` is refused:
  it is a single-party key, and with one signer the round-one mask would be
  the published one.
- **No public call panics on caller input** (`combine` on an empty signing
  set, `private_share` for an unknown party), and the accessors that expose a
  raw pairwise seed exist only in test builds.
- **The used-session set must be persisted with the share** -- documented on
  `KeyShare`: a share reloaded without it would answer a replayed session.

What the tests also showed: a dealer who sends one party a round-two message
from another ceremony (wrong share, wrong pairwise seed) is caught at signing,
by the round-three MAC check failing between exactly that pair -- so the DKG
is not verifiable, but its damage is a session that fails and points at who
to ask, never a signature that passes.

## Findings, reported rather than worked around

- **The reference's modulus does not meet the paper's Lemma 3.2 condition.**
  The reference uses Raccoon's `q = (63·2^18 + 1)(127·2^18 + 1)`, a product of
  two primes (the paper's asymptotic section asks for a prime). At `nu_t = 37`
  the lemma needs `floor(q / 2^37) = round(q / 2^37)`; here they are 4000 and
  4001. Worked out exactly: a coefficient of `A s + e` in a window of width
  49,807,361 just below `q` rounds with error about `2^37` instead of the
  lemma's `2^36`. The probability is 9.1 × 10^-8 per coefficient, about
  2.3 × 10^-4 per key, and one such coefficient moves the verification norm by
  far less than `B2`'s slack (the authors' own run sits at `n2 / B2 = 0.48`).
  Harmless in practice, but it is a departure from the stated condition, and
  the port follows the reference so that the vectors hold.
- **The DKG is secure only against parties who follow it.** It is not
  verifiable: a dealer who sends one party a share of a different polynomial is
  not caught. `threshold_tests.rs` shows the consequence is the one that can be
  tolerated -- a quorum including the victim produces no valid signature, a
  quorum without it still signs -- but it is a denial of service, and
  identifying the culprit needs lattice verifiable secret sharing, which is
  open research. The summed secret is also a sum of rounded Gaussians rather
  than one discrete Gaussian; the two are statistically close at these widths,
  but that is outside what the paper proves.
- **Shares are exchanged in process in the tests.** The REAL custody path
  already carries its frames over X25519MLKEM768 TLS with ML-DSA-87 channel
  binding (`custody-mpc::pq_auth`); wiring the TRaccoon rounds into that
  transport is not done, because nothing is to use them yet.

## Alternatives considered

- **Threshold ML-DSA.** The option that would make threshold custody sign
  *transactions*: its output is a plain FIPS 204 signature every node already
  verifies. Constructions exist (the "Threshold Signatures Reloaded" line of
  work by largely the same authors, among others), but for a handful of
  parties and, as of this ADR, without a peer-reviewed venue this repository
  could cite with confidence. The revisit trigger below is exactly this.
- **Ringtail (IEEE S&P 2025).** Two rounds instead of three, from LWE. A
  peer-reviewed alternative, but with no reference implementation to pin a
  port to bit for bit, which decision 2 treats as a requirement.
- **Keep refusing.** The brief allows a RESEARCH implementation once a
  peer-reviewed scheme is chosen; one is, so refusing would leave §7 undone.

## Consequences

- `threshold-lattice` compiles a working TRaccoon-128 with a dealerless keygen;
  `cargo nextest run -p maya-custody-mpc --features threshold-lattice` runs the
  known-answer test and the behaviour tests (any three of five sign with two
  offline; two cannot; a crash after round one aborts and a retry signs; a
  lying signer is caught in round three; a bad dealer costs availability, not
  security; a session id is never answered twice).
- Signature size and cost are the paper's order: about 13 KB and three rounds.
  Nothing on chain pays either.

## Revisit when

A threshold scheme whose output is a standard FIPS 204 signature is published
at a peer-reviewed venue with a reference implementation (then it replaces this
for anything that signs transactions), or lattice verifiable secret sharing
makes the DKG robust against a malicious dealer.
