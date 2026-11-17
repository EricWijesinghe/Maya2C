# 2026-11-15 — Signed tx header and header hash id (ADR-143)

Eric 2026-11-14 MCQ, "all format fixes". Branch `refreeze/tx-version-hash-id`, from `origin/master` `cd60b8cf`.

## Red (original sources, new test)

`cargo test -p custom-l1-node --test signed_header_red` (the transaction half of
`signed_header_tests.rs`, run against the unmodified `core/` sources):

```
test every_signed_message_names_its_wire_version_and_suite ... FAILED
test a_v5_signature_is_refused_when_presented_as_a_v7_0x30_envelope ... ok
test a_v7_0x30_signature_is_refused_when_presented_as_a_v5_transfer ... ok
assertion `left == right` failed: v5
  left: [0, 0]
 right: [5, 48]
test result: FAILED. 2 passed; 1 failed
```

The two cross-version tests passed on the old code too: the separate domains
already kept v5 and v7 apart. They stay as regression pins. The header half
failed to compile without `HashId` (`E0432: no HashId in core::block`).

## Green

```
signed_header_tests     5 passed
conformance             6 passed
consensus_conformance   1 passed
payload_tests          29 passed (digest re-pinned, prefix + length pass first)
ledger_fixture_tests    2 passed (fixture regenerated with MAYA_WRITE_LEDGER_FIXTURE=1)
suite_parity 11, suite_envelope_live 8, multisig 10, malleability 18,
chain_bound_signature 6, hybrid 24, hybrid_parity 8, block_bytes 10,
consensus 31, crypto 30 (1 ignored), exploit_replays 11, hash_migration_rehearsal 2
node --lib            542 passed, 1 ignored
maya-spec-ref           8 passed; spec-vectors --check clean
app-maya2c parity_tests 4 passed (host)
cargo xtask spec-coverage --strict: 64 rules, 166 vectors; 61 consensus rules covered, 0 gaps
node spec/verifier-ts/verify.ts: 0 mismatches
```

## Not run

- The whole node test suite, and workspace clippy and coverage, are left to CI.
- `apps/ledger-maya2c/tests/ledger_tests.rs` needs a device or speculos.

## Build note

The shared sccache server kept respawning about every 10 minutes, and each compile waited out its startup timeout. A red run took more than 3 hours that way. With `MAYA_NO_SCCACHE=1` the same build took 25 s.
