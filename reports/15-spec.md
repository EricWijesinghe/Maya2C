# 15 — Protocol specification, conformance and upgrades

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 15

> DONE WHEN: spec/ covers every mainnet-core rule; conformance vectors run in
> CI against the node and the independent verifier, both passing;
> spec-coverage shows zero uncovered consensus rules (or lists them as gaps);
> upgrade rehearsal and hash-migration rehearsal pass in sim/; reports/15-spec.md
> is complete.

| Condition | Result |
|---|---|
| spec/ covers mainnet-core rules | 44 rule IDs over 8 sections. Staking has none because it is not built. DAG-BFT has none because it is not wired in (ADR-015) |
| Vectors pass against the node | **yes**, 5 conformance tests (below) |
| Vectors pass against the independent verifier | **yes**: 50 cases, 0 mismatches |
| In CI | the node side runs in the `test` job; the new `spec` job runs `--check`, the verifier and coverage. **Not yet observed running on GitHub** |
| spec-coverage | 34 consensus rules covered, **7 listed as gaps** |
| Upgrade rehearsal | **passes**: 100 nodes |
| Hash-migration rehearsal | **passes**: 20 nodes, plus timing |

## 1. Specification

`spec/README.md` indexes eight sections, each with an owner role and a
version: encoding, transactions, state transition and root, fees, consensus,
staking, networking, crypto. Every rule is `- **ID** …`. Changes go through
`spec/mips/`, with a template covering motivation, specification, backwards
compatibility, security and test vectors.

## 2. The conformance loop

```
spec/tests/keys.json  (node-produced; addresses recomputed by both checkers)
crates/spec-ref       (reference: blake3 + serde_json, no node code)
  └─► spec/tests/{state_transitions, encoding, fees, headers}.json
        ├─► crates/node/tests/conformance.rs   real signatures, real StateDB
        └─► spec/verifier-ts/verify.ts          TypeScript, own BLAKE3
```

```
$ cargo test -p custom-l1-node --profile ci --test conformance -- --nocapture
test fee_vectors ... ok
test header_vectors ... ok
test encoding_vectors ... ok
test key_table_matches_the_node ... ok
17 state-transition vectors agree with the reference
test state_transition_vectors ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 15.78s

$ node spec/verifier-ts/verify.ts
verifier-ts (independent, signatures trusted from vectors): {"state_transitions":17,"encoding":9,"fees":17,"headers":3,"keys":4} cases, 0 mismatches

$ cargo run -p maya-spec-ref --bin spec-vectors -- --check     # exit 0: committed vectors are current
```

The TypeScript BLAKE3 was checked against the official test vectors: 8
lengths from 0 to 8,192 bytes, plus 5 derive-key vectors. The expected values
were computed with the Python `blake3` package, not recalled. It was also
compared with that package on a 13,000-byte input.

**Both checkers fail when they should.** One `post_root` was tampered with
and both were rerun:

```
MISMATCH transfer-basic: post_root                                         (verifier-ts)
assertion `left == right` failed: transfer-basic: post_root                (node)
```

**What the suite found on its first run.** The spec draft said a v5 txid was
`blake3(signing_bytes)`. The node hashes both signatures too. The encoding
vector failed with `signed-transfer: txid`, and the spec, reference and
verifier were corrected (ENC-8). Second, six vectors disagreed in TypeScript
because `JSON.parse` rounds integers above 2^53. Every u64 in the vectors is
now a decimal string (ADR-020 §3). Neither was a node bug; both were bugs in
the spec's first draft that the process exists to catch.

**What the verifier does not check:** signatures. It trusts each vector's
`signature` flag, and its output says so. ML-DSA and SLH-DSA are covered by
NIST ACVP vectors (`crates/crypto-pq/tests/acvp_tests.rs`).

## 3. Rule coverage

```
$ cargo xtask spec-coverage
…
44 rules, 71 vectors; 41 consensus rules covered, 0 gaps:
```

The seven gaps this section used to list were closed on 2026-09-28, from
`spec-ref`'s independent reference (`crates/spec-ref/src/consensus.rs`),
replayed by `crates/node/tests/consensus_conformance.rs` and
`conformance.rs`:

| Rule | Vectors |
|---|---|
| CON-4 state root must match execution | a declared root that matches and one that does not (`state_transitions.json`), through `apply_block_checked` |
| CON-5 retarget | on time, slow, fast, both clamps, the limit cap, and a declared target that disagrees |
| CON-6 work check | a hash below, equal to and above the target (the comparison; the `argonblake-pow` hash itself has no vector) |
| CON-7 most work | the work of a target, of the zero target (saturates) and of the easiest, and a short hard branch beating a long easy one |
| CON-8 prune horizon | above, at and below it |
| ROOT-5 extra layers | the accounts-only roots, which the node must reproduce with every layer present but empty |
| TX-4 v7/v8 verify only via `verify_at` | real ML-DSA-87 v7 and 2-of-3 v8 frames built by the node; `verify` refuses them, `verify_at` accepts, a changed frame fails |

The TypeScript verifier handles CON-4 but does not read `consensus.json`.

CRY-1/2/3 count as covered through *external vectors* (ACVP, suite parity,
suite registry tests). The tool checks each named path exists.

## 4. Versioning and activation

`crates/node/src/upgrade.rs` adds:

- `GenesisConfig::protocol_upgrades` (optional, hashed into nothing, so
  existing ids and roots are unchanged);
- `SUPPORTED_PROTOCOL_VERSION = 1`;
- `Chain::insert_block` refusing any block at or past the first unsupported
  version's height with `UpgradeRequired`, whose message is
  "upgrade required before height H: protocol version V activates there and
  this binary supports up to version 1";
- a start-up check in `maya2c-node` for a tip already past that height.

```
$ cargo test -p custom-l1-node --profile ci --test upgrade_rehearsal -- --nocapture
test the_halt_message_names_the_height ... ok
100 nodes: 70 upgraded followed to height 40; 30 late halted at 20 with UpgradeRequired, then caught up after upgrading
test late_nodes_halt_at_h_and_catch_up_after_upgrading ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.81s
```

Each node is a real `Chain` on its own RocksDB. A halted node's state root
equals its tip header's. After the upgrade it reopens the same data directory
and reaches the producer's tip.

**Deviations from the brief, stated:**

- Blocks do **not** carry a protocol version field. The version at a height is
  a function of the schedule; a header field would change the 144-byte layout
  (ADR-020).
- The schedule lives in genesis, so governance cannot move it yet.
- The rehearsal lives in `crates/node/tests/`, not `sim/`, because `maya-sim`
  is dependency-free by design and cannot link the node.

## 5. Crypto future-proofing

`docs/CRYPTO_WATCH.md` covers the suites in use, what to watch (FIPS 206, the
HQC standard, the signature on-ramp, cryptanalysis, IETF), a six-step
deprecation lifecycle with minimum durations, and the hash-migration
procedure. The page states that no standards body was consulted live.

```
$ cargo test -p custom-l1-node --profile ci --test hash_migration_rehearsal -- --nocapture
20 nodes re-committed 4 accounts to one SHA3-256 root; bridge binds it to the BLAKE3 root under both hashes
test every_node_recommits_to_the_same_new_root_and_proofs_switch_at_the_boundary ... ok
re-commit 1,000,000 accounts under BLAKE3: 0.38 s (…; build profile with debug assertions)
re-commit 1,000,000 accounts under SHA3-256: 56.63 s (…; build profile with debug assertions)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 57.13s
```

The SHA3 figure is 150x BLAKE3's under a profile whose dependencies are
meant to be optimized. It looks like an unoptimized Keccak and is **not** a
representative cost; it is recorded, not explained away. It was not
investigated further in this session.

## 6. Not done

- §5 state-migration framework (versioned steps, dry run, rollback): **not
  built.**
- §7 API stability: no semver policy document and no `cargo-semver-checks`
  job. The crates are unpublished, so there is no baseline to diff against
  except a git revision.
- A devnet replay through the verifier: only the vector suite is replayed.
- `docs/SECOND_CLIENT.md` states what a second full client needs. The honest
  answer is that it would still need to read Rust for the VM, the extra state
  layers and consensus.
