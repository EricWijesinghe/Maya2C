# ADR-013: The suite envelope and multisig are live from genesis

**Status:** Accepted
**Date:** 2026-09-27
**Amends:** ADR-007 §7 (activation), ADR-011 (multisig class)

## Context

Master Prompt 2 §1 asks that *every transaction envelope carries a suite ID*,
and §7 that m-of-n ML-DSA multisig be *REAL now, enforced by on-chain policy*.
Both were built — wire v7 (`core/suite_tx.rs`) and wire v8
(`core/multisig_tx.rs`) — and both were dark: `SUITE_ENVELOPE_ACTIVATION_HEIGHT
= u64::MAX`, so `verify_at` refused them at every height and the height-less
`Transaction::verify` refused them outright. `reports/02-crypto.md` reported
multisig as RESEARCH for that reason.

ADR-007 wrote down what had to be true before the height could move. All three
are now true, and each is checked by code rather than asserted here:

| Precondition (ADR-007) | Where it now holds |
|---|---|
| The address commits to the suite | `crypto::suites::suite_address` = `BLAKE3-derive-key(suite ‖ pk)`; a multisig address is the digest of the whole policy (`multisig_address`) |
| The governance default is read by admission | `network::mempool::Mempool::validate` builds its policy from `ParameterKey::DefaultSignatureSuite` through the committed table |
| Consensus call sites use `verify_at` | `state::db` `stage_transaction` (apply), `state::stateless::verify_block`, `state::threat_exec::frame_fails_signature` |

## Decision

1. **`SUITE_ENVELOPE_ACTIVATION_HEIGHT = 0`.** Zero rather than a later height
   for the reason ADR-012 gave for hash locks: before this change every v7 and
   v8 frame was refused at every height, so no block any chain accepted can
   contain one, and no history is reinterpreted. v5/v6 hybrid frames remain
   valid for ever; they are `0x30` envelopes without the header.
2. **Consensus verifies under one constant policy.**
   `crypto::suites::verification_policy()` is the genesis schedule under
   mainnet rules. It reads no state and no configuration:
   - `SuitePolicy::may_sign` depends on the deprecation schedule and the
     height, never on the default, so a governance vote cannot change whether
     a historical signature verifies.
   - The network is fixed at `Mainnet` because `Devnet` admits `0x01`
     (Ed25519, zero post-quantum bits). A node's config file must never decide
     which signatures its chain accepts. Consequence: Ed25519 cannot sign a
     suite-tagged frame on any network. It stays in the registry for tooling.
   - A stateless verifier holds an accounts witness and no governance column
     family; a rule that read state is one it could not evaluate.
3. **The governance default chooses what wallets sign with, not what
   verifies.** Admission reads it (and fails loudly if the table names a byte
   that is not a permitted suite), then applies the *same* admissibility test
   consensus does. Refusing at relay a suite that blocks still accept would
   strand valid transactions without changing any block's validity.
4. **Admission judges at the tip's successor**, the earliest height the
   transaction could execute at. It is advisory: `stage_transaction` re-checks
   at the block's real height. A store with no tip yet judges at genesis.
5. **`verify_block` takes the height**, because a header carries none and the
   height decides which schedule applies.
6. **`Transaction::verify` keeps refusing v7 and v8.** It is the safety net for
   any path that forgets to pass a height: such a path fails closed.

## Alternatives considered

- **A later activation height.** Buys nothing: there is no running network
  whose history a height would protect, and the three preconditions are met.
- **Policy from the governance table in consensus.** Would make validity a
  function of later votes, and is not evaluable statelessly. Rejected.
- **Network from node config.** A fork waiting for a misconfigured node.
  Rejected.
- **Relay-time floor at the default's security level.** Would give governance
  a sharper lever, and would strand v5/v6 hybrid transactions (128 PQ bits)
  the moment the default moved to ML-DSA-87 (192). Rejected.

## Consequences

- Multisig and the Ledger app's suite-`0x10` signing are REAL: reachable by
  consensus and tested through it
  (`crates/node/tests/suite_envelope_live_tests.rs`).
- Invariant 31 is restated: v7/v8 verify from genesis under
  `verification_policy`, and `Transaction::verify` never accepts one.
- A deprecation schedule, when one is written, lives in
  `verification_policy()` as code, reviewed as a consensus change — not as a
  governance parameter.

## Revisit when

A second network (testnet with different suites) needs its own schedule. The
answer then is a genesis-committed network identifier, never a config flag.
