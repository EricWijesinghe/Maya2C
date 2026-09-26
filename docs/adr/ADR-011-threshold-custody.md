# ADR-011: Threshold custody — on-chain multisig now, threshold lattice signing later

**Status:** Accepted
**Amended by:** [ADR-013](ADR-013-suite-envelope-activation.md), 2026-09-27 — the multisig class: the envelope is live from genesis.
**Date:** 2026-09-21

## Context

The brief (Master Prompt 2 §7) asks for three things:

1. **REAL now:** m-of-n ML-DSA multisig enforced by on-chain policy.
2. **RESEARCH, behind a feature:** true threshold lattice signing, a
   dealerless keygen plus partial-signature aggregation, where the full key
   never exists in one place. It is to be enabled only after a peer-reviewed
   scheme has been chosen in an ADR.
3. Shares travel over mutually authenticated PQ Noise/TLS, tested with a
   3-of-5 signing ceremony under simulated node failures.

What the tree already had: `custody-mpc`, a dealerless Pedersen-VSS vault
that **reconstructs** the 32-byte chain key for the length of one signature,
and says so in its first paragraph. It is custody of a secret, not threshold
signing, and it fails requirement 2's "never in one place" by construction.

## Decision

### 1. Multisig is a policy account, not a new signature

`maya_crypto_pq::multisig`: a policy is `m` plus an ordered list of up to
16 suite-tagged keys. Construction refuses `m = 0`, `m > n`, a repeated key,
and any suite with no post-quantum security. Verification needs at least `m`
approvals, in strictly increasing index order, so a repeated signer cannot be
represented. Every approval presented must verify, so a relayer cannot pad a
valid quorum with garbage.

On the node, wire version 8 (`crates/node/src/core/multisig_tx.rs`) carries
the policy, the approvals and the usual body. The **sender is the policy's
address**: a domain-separated BLAKE3 hash of its digest. Spending from the
account therefore means presenting exactly the policy the address commits
to, and every approver signs bytes that include that whole policy. That is
the "on-chain policy": the chain holds no registry to update and no admin
key, because the policy *is* the account.

Each member signs with an ordinary registry suite, verified against NIST
ACVP vectors. So the only new code is counting, and no key ever exists
anywhere but with its holder. That meets requirement 2's goal without its
cryptography, at the price of `m` signatures on the wire: roughly
3 × 4,627 B for a 3-of-5 over ML-DSA-87.

**Dark on the v7 gate.** Multisig is a consensus change, so it goes behind
the same `SUITE_ENVELOPE_ACTIVATION_HEIGHT` (`u64::MAX`) as ADR-007's
envelope: `Transaction::verify` refuses it and only `verify_at` can accept
it. Every listed key's suite must be admissible there, not just the
approvers', so an account that names a retired suite cannot hide it behind
a quorum that avoids that key. The code and its tests are real. Whether it
is live is the activation decision ADR-007 already defers.

**The txid excludes approvals.** Any quorum of a policy authorizes the same
spend. If the txid hashed approvals, one spend would have as many valid ids
as it has quorums, and anyone holding a spare approval could re-encode a
broadcast transaction under a new id, orphaning every child that spends it
by the old one. So a v8 txid is BLAKE3 over the signing bytes alone, the way
a segregated-witness txid omits the witness. Pinned by
`every_quorum_of_one_spend_has_one_txid`.

**Before the gate moves** (review findings, for the activation ADR):

- Suite-tagged and multisig frames need a size or cost weight. Sixteen
  SLH-DSA-SHAKE-256f approvals are ~800 KB of verification-heavy signature.
- No signing domain in any wire version binds a chain id. This predates
  v8, but it matters before any fork or sidechain shares keys.

### 2. The custody TLS hop is X25519MLKEM768

`custody-mpc`'s TLS (rustls on `ring`) negotiated classical groups only. The
shares themselves were already sealed under ML-KEM-768, but the metadata —
who took part in which session, and when — was open to
record-now-decrypt-later. `custody_mpc::pq_kx` implements the hybrid group
of draft-ietf-tls-ecdhe-mlkem (codepoint `0x11EC`) as a rustls
`SupportedKxGroup`, reusing `crypto-pq`'s ML-KEM-768 and `curve25519-dalek`.
It is the **only** group offered, and TLS 1.3 is the only version, so a
classical-only peer fails the handshake instead of downgrading.

Authentication stays classical: `webpki` verifies no ML-DSA certificate.
Forging it needs a quantum adversary active during the handshake. Recording
the traffic is not enough.

The combined 64-byte secret is handed to rustls as a plain `Vec`, and
rustls's `SharedSecret` does not zeroize on drop. Every rustls key-exchange
provider does the same, and there is no fix short of forking rustls. It is
recorded here rather than left as an unstated exception to the
secrets-are-zeroized rule.

### 3. Threshold lattice signing is an interface, and it refuses

The `threshold-lattice` feature compiles `custody_mpc::threshold`: the
`ThresholdScheme` contract (dealerless keygen, partial signatures,
aggregation into one signature that an existing suite verifies) and
`activate()`, which returns `NoThresholdScheme` while `SCHEME` is `None`.
Nothing implements the trait.

**No scheme is chosen.** Candidates, to be re-read at the time of choosing
rather than trusted from this list:

| Candidate | Output | Why not yet |
|---|---|---|
| Threshold Raccoon (del Pino, Katsumata, Maller, Mouhartem, Prest, Saarinen; Eurocrypt 2024) | Raccoon signature | Not a registry suite; Raccoon itself was not selected by NIST |
| Ringtail (Boschini, Kaviani, Lai, Malavolta, Takahashi, Tibouchi; IEEE S&P 2025) | Its own LWE signature | Not a registry suite; young |
| Threshold ML-DSA constructions (2025, several groups) | Plain FIPS 204 | The appealing one — no consensus change — but the published constructions target small party counts, and none has an audited implementation |

The criteria for choosing one:

- The output verifies under a registry suite with the unmodified verifier.
- There is a peer-reviewed security proof in the threshold setting.
- There is an implementation that can be audited.
- Keygen is dealerless.

When a scheme meets them, a new ADR supersedes this section, sets `SCHEME`,
and implements the trait. Until then the feature is compiled in CI so the
contract cannot rot, and no default build enables it.

## Consequences

- A 3-of-5 exists three ways, each tested with two members down:
  - `crates/crypto-pq/tests/multisig_tests.rs`, the policy;
  - `crates/node/tests/multisig_tests.rs`, v8 on the node;
  - `crates/custody-mpc/tests/tls_tests.rs`, the vault over PQ TLS. It also
    covers a crash plus a corrupted share (signs), three failures (refuses),
    and a classical-only peer (refused).
- `features.toml`:
  - multisig is class RESEARCH, status `working`. The brief says "REAL now",
    but this repository's REAL means reachable by consensus, and v8 is dark
    on the same gate as the v7 envelope. It becomes REAL when that gate
    activates, with no code change;
  - threshold lattice signing is RESEARCH, status `stub`.
- A multisig account's approvals are public: who signed is on chain. That
  is an audit trail, not a leak, for the treasury and custody uses this
  targets.
