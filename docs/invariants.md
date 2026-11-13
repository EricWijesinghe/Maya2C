# Critical Invariants

Thirty-one live properties this tree must not lose (numbered 1-36; 20-23 are retired and keep their numbers, 35 is held by a proposal), each with the reason it
exists. They are numbered, the numbers are referenced from code comments and
from `docs/adr/`, and **a number is never reused**: if an invariant is
retired, its entry stays with the retirement recorded.

An invariant earns a number only once a test pins the behaviour. A rule
nobody checks is a comment, and it belongs beside the code it describes.

Moved out of `CLAUDE.md` on 2026-09-20, verbatim and unrenumbered, so that
file could stay short enough to be read in full. See
[adr/ADR-001-workspace-layout.md](adr/ADR-001-workspace-layout.md) for the
phase that did it.


1. **`ledger-math` must never gain a C/C++ dependency.** Kani compiles a crate
   with its full dependency graph; pulling in RocksDB breaks model checking.
2. **`crypto-pq` must remain the crate that instantiates `slh-dsa`.** Measured:
   4468 ms → 140 ms signing when the *instantiating* crate is optimized rather
   than the generic one. Moving the instantiation moves the optimization.
3. **`cuda-miner`'s `cuda` feature stays default-off** so
   `cargo build --workspace` works on GPU-less runners.
4. **`fips204` compiles only the `ml-dsa-65` parameter set.** A compiled-in set
   is a set someone can select by accident. *Extended by 29 (2026-09-21):* still
   true of `fips204`; ML-DSA-87 arrives through `ml-dsa` in the closed suite
   registry, which is the form of the same rule that survives crypto-agility.
5. **`[profile.dev.package.*]` overrides are load-bearing, not tuning.** Without
   them `cargo test` reads as hung, not slow. Do not "clean them up".
6. **`dex` must stay dependency-free**, for the same reason as `ledger-math`.
   The visible cost is that it cannot hash, so pair and share-asset identifiers
   are derived in `custom-l1-node` and passed in as opaque bytes. That is the
   price of the boundary, not an oversight to tidy up.
   *Amended 2026-10-13 (ADR-048):* "dependency-free" now reads "no dependency
   outside the workspace's proved crates". `dex`, `ledger-math`, `fee-market`,
   `staking` and `governance` depend on `maya-amount` alone — itself `no_std`
   with an empty `[dependencies]` — so each Kani graph is still all
   first-party Rust. The reason is unchanged; only its letter moved.
7. **A trade that merely *loses* is a no-op, never an `Err`.** A missed slippage
   bound, a lost arbitrage race, a batch the pool cannot price: nonce advances,
   nothing moves. A failing transaction fails its whole block here, so making
   any of these an error hands every trader a way to void a block. See
   `docs/dex.md`; `crates/node/tests/dex_tests.rs` pins it.
8. **Every trading record's prior value goes in the undo journal.** A reorg that
   left a pool holding the abandoned chain's reserves is not a detectable
   corruption — it is two plausible numbers that go on quoting a price. The same
   applies to the oracle's `o:` records.
9. **Oracle freshness is measured in block height, never in timestamps.**
   `chain.rs` reads `header.timestamp` only for difficulty retargeting; there is
   no future-drift bound and no median-time-past, so a miner may write any
   `u64`. A timestamp-based freshness check would read as safety and provide
   none. See `docs/oracle.md`.
10. **The VRF suite octet is `0x03`.** `0x04` is `…-SHA512-ELL2`, the same curve
    and hash under a different hash-to-curve map. Using it produces proofs that
    are internally consistent, pass every round-trip test, and match no other
    implementation on earth. `crates/vrf/tests/rfc9381_vectors.rs` is what catches it —
    do not "simplify" those vectors away.
11. **The oracle is optional and absent by default.** A genesis file without an
    `oracle` section produces no `o:` records and the state root the chain would
    have had without the subsystem. Introducing the chain's only trusted party
    is a decision somebody writes down.
12. **Governance must not be able to make governance unsafe.** The quorum floor,
    the approval floor, the minimum voting period, and the minimum timelock are
    compiled into `crates/governance/src/limits.rs`, appear in no `ParameterKey`, and
    are reachable by no transaction. Every governable value carries a hard range
    checked *twice* — when proposed and again when executed, because a release
    between the two could have tightened it.
13. **No governance key's value is a program.** Native code is never fetched from
    chain state and run. A rule change either moves a number or flips between
    two implementations the binary already ships, as `crypto/dag/registry.rs`
    already does with `activation_height`. See `docs/governance.md`.
14. **Nothing on the telemetry dashboard is verified.** Every figure but
    difficulty is an unauthenticated claim. Heights and propagation are medians
    so one liar cannot set them, reports replace rather than accumulate, and
    claims outside a hard bound are rejected rather than clamped — a clamped
    report is a number the reporter never sent. See `docs/telemetry.md`.
15. **The telemetry map is country-granular and has a floor.** No address is
    stored, no coordinate exists anywhere in the crate, and a country with
    fewer than `MIN_REPORTERS` is folded into `ZZ`. One miner in a small
    country is an individual, not aggregate data. `MIN_REPORTERS` is a
    compiled-in constant and no configuration key, because tuning it to 1 to
    "see more detail" is the failure it prevents.
16. **The faucet's two rate-limit buckets are independent.** Keying on the
    `(IP, address)` pair is not a limit: keypairs are free, so one IP with a
    thousand fresh addresses is a thousand payouts. Both buckets must clear,
    and both are consumed only if both pass. The daily cap, not the limiter, is
    what bounds a distributed drain. See `docs/faucet.md`.
17. **A governed value is read from state, never from a `const`.** The constants
    that remain (`MAX_FILLS_PER_BLOCK`, `DEFAULT_PROTOCOL_FEE_BPS`, …) are the
    parameter table's *defaults*. Reading one directly at a call site silently
    un-governs that rule.
18. **`custody-mpc` reconstructs the vault key in one place, and that is the
    design, not a defect to fix quietly.** A Maya2C signature is a hybrid pair
    and both halves must verify; there is no threshold construction for
    SLH-DSA at all, and `fips204` exposes nothing that decomposes into partial
    ML-DSA signatures. So the crate protects the 32-byte *chain key* — which
    `signing_key_from_seed` expands into both halves — rather than thresholding
    either signature. The combiner holding the key for the length of one
    signature is the whole cost, it is stated at the top of `crates/node/src/lib.rs`, and
    anything that quietly relaxes it (a "partial signature" API, a second
    combiner, caching a reconstructed seed) breaks the only claim the crate
    makes. See `docs/custody-mpc.md`.
19. **Every reconstruction is checked against the vault's commitment before a
    key is derived from it.** Interpolating from too few shares does not fail —
    it returns a different secret, silently. `vss::check_opening` is what turns
    a short quorum, a corrupted safe, or an inconsistent dealer into an error
    instead of a signature under a key that owns nothing. It is the reason
    `interpolate_opening` returns the blinding factor alongside the secret, and
    the reason there is no public way to obtain one without the other.
20. *[Retired 2026-09-21 with `crates/zkml` and `crates/zkml-prover` — ADR-008. Kept for the record; the number is not reused.]* **No ONNX runtime on the consensus path.** The node depends on `maya-zkml`,
    which is the verifier only; `tract-onnx` lives in `maya-zkml-prover`, which
    the node takes as a dev-dependency and nothing more. A separate crate rather
    than a feature, because a feature can be switched on by any crate in the
    graph through unification and a crate the node does not depend on cannot.
    Most ONNX models are floating point, and a float in a consensus rule is a rounding
    mode two validators can disagree on. Verification checks a proof; nothing
    in a block runs a model. See `docs/zkml.md`.
21. *[Retired 2026-09-21 with `crates/zkml` and `crates/zkml-prover` — ADR-008. Kept for the record; the number is not reused. 21 still binds `host_verify_zkml_proof`, which stays in the VM ABI and answers "no verifier".]* **A host function that does native work charges fuel for it, first.** Gas
    is wasmtime fuel and cannot see native work, so `host_verify_zkml_proof`
    charges a *measured* price (`crates/vm/src/zkml.rs`, calibrated by
    `crates/vm/tests/fuel_calibration_tests.rs` and `crates/zkml-prover/benches/verify.rs`) before
    it reads a byte, and traps out-of-fuel before the verifier runs. There is
    deliberately no tensor host function: guest wasm is priced exactly by the
    fuel meter, and a hand-set per-MAC price would be consensus-critical and
    wrong on some machine.
22. *[Retired 2026-09-21 with `crates/zkml` and `crates/zkml-prover` — ADR-008. Kept for the record; the number is not reused.]* **zkML stays dark until its SRS is real and gas is capped.** The SRS in
    `crates/zkml/src/srs.rs` is derived from a public seed, so anyone can forge proofs;
    and no cap bounds a call's `gas_limit`, so a fuel price bounds nothing
    absolutely. `ZKML_ACTIVATION_HEIGHT` is `u64::MAX`, and
    `state::zkml::check_setup` refuses mainnet the moment it is anything else
    while `SRS_IS_TRUSTED` is false.
23. *[Retired 2026-09-21 with `crates/zkml` and `crates/zkml-prover` — ADR-008. Kept for the record; the number is not reused.]* **Every constraint in `crates/zkml/src/circuit.rs` has a test that fails without
    it.** Negative tests hand the circuit a lie that is *consistent* — everything
    downstream recomputed — so only the guard under test can refuse it. A lie
    left inconsistent is caught by some other constraint, and the test then
    passes with its own guard deleted; that happened, and the mutation sweep in
    `docs/zkml.md` is how it was found. Changing the circuit means re-running
    that sweep.
24. **A block's id and proof of work cover its transactions, and its declared
    state root is checked.** `BlockHeader::tx_root` (bytes 112..144, after the
    nonce so `NONCE_RANGE` never moved) is a `state::merkle` root over
    `transaction_leaf(txid)`. `Chain::insert_block` calls `check_tx_root`
    *first*, ahead of the duplicate check and before anything is stored: a
    mismatched body filed under an honest id would turn the genuine block into
    a `Duplicate`, which is censorship by one relay. `apply_block_journaled` —
    the chain's only apply path — refuses a `state_root` that execution does not
    produce. Block producers take both roots from `Chain::candidate_block` and
    never from `state_root()`, which is the *pre*-block root. Before this, one
    block id could carry two transaction lists (`crates/node/tests/chaos_simulator.rs`
    replays that attack). Do not add an unchecked apply path to `Chain`.
25. **Every persisted consensus record is under the state root; everything
    else is on an explicit local-only list.** `state::commitments` holds the
    lists:
    - `RECORD_LAYERS`, the one source for the generic-record prefixes;
    - `committed_prefixes()`;
    - `LOCAL_ONLY_PREFIXES` (`undo:`, `blk:`, `bdelta:`). `bdelta:` is the Mesh
      balance-change record (`state::balance_changes`): derived from the block's
      own before/after pair, so it is not consensus state, and it is deleted with
      the undo journal on reorg.
    - The upgrade schedule (`up:`, ADR-082) is a committed layer, not local-only.
      Its writes are overlay records, so `capture_undo` records their prior values
      and a reorg restores them.

    Until 2026-09-12 contract code, contract storage, the nullifier set, and the
    shielded pool's anchor window and balance were all outside the root.
    Nothing checked them, a snapshot could forge them, and a reorg did not even
    restore contract storage. A new prefix goes into those lists and into a
    layer, and the undo journal records its prior value. Otherwise
    `uncovered_keys()` fails the subsystem's tests, and the `write_overlay`
    debug assertion fails any test that writes a stray record.
26. **Blocks and the state they produced commit in one batch.** The block
    store (`state/blocks.rs`) lives in the state's RocksDB:
    - `apply_canonical` writes the canonical-index entry and the tip pointer
      in the same `WriteBatch` as the block's state;
    - `revert_canonical` does the same for a revert.

    `Chain::open` rebuilds from it and adopts a heavier stored branch, which is
    how a crash mid-reorg recovers. `seed_state` runs on a **fresh** database
    only: re-seeding an evolved one overwrote it, which is why no node could
    restart before 2026-09-12.
27. **No block body is deleted before a verified copy exists, and a pruned
    node never reorgs below its horizon.** `prune_round` archives to every
    store and reads every copy back before `Chain::prune` deletes anything.
    `Chain::prune` refuses a batch the active chain no longer holds.
    `insert_block` and `reorganize` refuse to reach at or below
    `prune_horizon`, before anything is reverted. Pruning is opt-in: an
    archive node has no horizon. A snapshot is imported only if it reproduces
    `header(H).state_root`; otherwise it is wiped.
28. **The guard refuses invalid blocks and halts modules, but never halts a
    transfer.** `StateDB::stage_block` is the one place an overlay is built, so
    the hook at the end of it covers all four commit paths — including
    `preview_root`, which is why an honest miner refuses to *build* a bad block
    rather than minting one the network rejects. A conservation failure is a
    block-level `InvariantViolation`, refused like a wrong state root, with no
    breaker written because there is no committed state to protect; an anomaly
    is a *judgement*, so the block commits and only the module halts, for
    `BREAKER_BLOCKS` = 100. `Module::of(TxKind::Transfer)` is `None` and the
    match has no wildcard arm, so peer-to-peer payments are ungated by
    construction and adding a `TxKind` is a compile error until somebody
    assigns it. The guard reads only committed state and the block — no clock,
    no configuration, no node-local value — and the anomaly checks run in a
    fixed order, because a breaker that trips on one node and not another is a
    chain split. Every anomaly threshold carries a floor or a tolerance chosen
    so the breaker cannot be *bought*: `SHIELDED_DRAIN_FLOOR` exempts a pool too
    thin for its percentage to mean anything, because a rate with no floor is a
    lever anyone can pull to halt the module for the price of one fee.
    `g:guard:` sits under the governance prefix (asserted at compile time) so it
    is already under the state root per 25.
    `crates/node/tests/exploit_replays.rs` pins it; see
    [docs/invariant-guard.md](invariant-guard.md).
29. **Only parameter sets named in the signature-suite registry are compiled
    into a signature path, and every FIPS one has NIST known answers.**
    `SuiteId` is a closed `#[repr(u8)]` enum; an unknown byte is a decode error,
    never a fallback, because a suite some nodes know and others do not is a
    fork. Adding a suite is a code change plus ACVP vectors, never a governance
    act — governance only chooses among compiled suites. Pinned by
    `crates/crypto-pq/src/suite/tests.rs`
    (`every_byte_either_names_a_suite_or_is_refused`) and
    `crates/crypto-pq/tests/acvp_tests.rs` — ADR-007.
30. **Suite `0x30` is the node's hybrid, byte for byte.** Same seed domains, same
    deterministic FIPS 204/205 signing, same `ml_dsa_pk ‖ slh_pk` and
    `ml_dsa_sig ‖ slh_sig` layout — so every account and every historical
    signature is already a valid `0x30` envelope and the envelope can land
    without migrating anyone. `fips204` (node) and `ml-dsa` (suite) must stay
    the same function of the same seed; `crates/node/tests/suite_parity_tests.rs`
    pins keys and signatures across both — ADR-007.
31. **Suite-tagged (v7) and multisig (v8) transactions verify only through
    `verify_at`, under `crypto::suites::verification_policy()`, and
    `Transaction::verify` never accepts one.** `verify` has no height, so it
    refuses outright: a path that forgets to pass one fails closed. The policy
    is a constant — the genesis schedule under mainnet rules — so neither a
    governance vote nor a node's config can change which signatures a block's
    validity rests on. `SUITE_ENVELOPE_ACTIVATION_HEIGHT` is 0 (ADR-013).
    Pinned by `crates/node/tests/suite_parity_tests.rs` and
    `crates/node/tests/suite_envelope_live_tests.rs` — ADR-007, ADR-013.
32. **A vault account moves at most its `limit` per window of `delay_blocks`
    without a delayed, guardian-cancellable request, and by no door but a
    plain transfer or a vault action.** Outputs to the fee collector count
    toward the limit above 4x the required fee. Before review closed that
    door, one transaction could send a vault's whole balance to the
    collector. A settled request is deleted, so it cannot be settled twice,
    and open withdrawals are counted by the conservation guard. Pinned by
    `crates/node/tests/vault_tests.rs` (including
    `the_fee_collector_is_not_a_way_around_the_limit` and
    `the_limit_bounds_a_window_not_a_transaction`) — ADR-030.
33. **A block whose wire form is over `MAX_BLOCK_BYTES` (5 MiB, provisional) is
    refused on import and on every apply path, and the selection that builds a
    block passes over any transaction that would take it over (first-fit).** The
    choice is a function of the order and the cap, so every node makes the same one, and what is left out stays with
    its proposer for a later block. Pinned by
    `crates/node/tests/block_bytes_tests.rs` (including
    `a_block_over_the_cap_is_refused_on_import` and
    `a_transaction_left_out_is_applied_once_in_a_later_block`) — ADR-072.
34. **A scheduled protocol version or suite sunset takes effect no sooner than its
    compiled lead, and no entry is moved earlier, removed, or placed before one
    already written.** Governance's `ScheduleUpgrade` needs
    `MIN_UPGRADE_LEAD_BLOCKS` (30 days at one block per second) between the last
    height its proposal can execute at and its activation. The security council's
    `SunsetSuites` needs `COUNCIL_SUNSET_LEAD_BLOCKS` (1,172,800 blocks, about
    13.6 days) and may schedule nothing else. Neither can move funds or mint: a
    schedule entry switches on a dark feature or retires a suite. Both leads are
    compiled, so no proposal shortens them: `MIN_UPGRADE_LEAD_BLOCKS` in
    `crates/governance/src/limits.rs` (invariant 12), and
    `COUNCIL_SUNSET_LEAD_BLOCKS` in `crates/node/src/state/upgrades.rs`, derived
    as `vault::MAX_DELAY_BLOCKS + agility::MIN_MIGRATION_WINDOW`
    (1,000,000 + 172,800) so a vault withdrawal a sunset announcement starts can
    finish before the sunset height (ADR-082).
    Pinned by `crates/node/tests/upgrade_schedule_tests.rs`
    (`a_schedule_upgrade_one_block_short_of_the_lead_is_refused_at_proposal`,
    `a_new_entry_must_lie_after_every_entry_already_written`) and
    `crates/node/tests/council_pause_tests.rs`
    (`a_council_sunset_is_scheduled_only_at_the_council_lead_and_never_as_a_version`)
    — ADR-082.
36. **No float and no model output in a consensus rule.** A float is a rounding
    mode: two validators on different CPUs, compilers or flags can round one
    expression differently, and one ULP of disagreement in a state root is a
    fork. A model's output is the same hazard one step removed, and a model
    that decides a block, a payout or a parameter has moved a decision out of
    the rules every node can re-derive. Job outputs may be stored as data; a
    model may not decide anything. The integer fee model in
    `crates/fee-market/src/model/` is compiled in and dormant:
    `neural_activation_height` is `u64::MAX` in `crates/node/src/genesis.rs`.
    Giving it a finite height is an amendment to this invariant, not a
    parameter change. The rule lived under invariant 20, which retired with
    `crates/zkml` (ADR-008) and left it as prose nobody checked (owner
    decision, Eric 2026-11-14). `cargo xtask float-check` takes its scope from
    the node binary's link graph: `bins/maya2c-node` and every path crate it
    reaches through normal, build and target-specific dependencies
    (`xtask/src/float_scope.rs`), so a crate the node starts to link is
    scanned by default. It lexes every non-test source file there, macro
    bodies included, and fails on an `f32`/`f64` ident, an ident with
    `f32`/`f64` as a `_` segment (`as_f64`, `to_f32`), `Float`, or a float
    literal. Six linked paths are excluded, each with a written reason
    (`EXCLUDE`: the node's `rpc`, `network` and `metrics` modules, `archive`,
    `iso20022`, `hal/ebpf-net`). One allowlist entry holds the
    finality-latency metric in `bins/maya2c-node/src/bft.rs`, and lists the
    exact tokens allowed, so any other float in that file still fails. The
    check also fails if a named consensus path stops being scanned
    (`MUST_SCAN`), and if an ML runtime (`tract-*`, `ort`, `onnxruntime`,
    `candle-*`, `burn`, `tch`, `maya-neural-fee`, `maya-zkml-prover`) is
    anywhere in the node's dependency closure, walked through `Cargo.lock`
    (transitive, renamed and target-specific dependencies included). Runs in
    CI's `reality ledger` job. Pinned by `xtask/src/float_check.rs`
    (`the_consensus_crates_hold_no_float_and_no_model`,
    `a_float_type_or_literal_is_found_anywhere_in_code`,
    `a_test_field_parameter_variant_or_arm_hides_only_itself`) and
    `xtask/src/float_scope.rs`
    (`an_ml_runtime_two_hops_away_is_in_the_lock_closure`). Not covered: a
    float reached by type inference with no float token in the scanned file
    (`"0.5".parse()` into an inferred type, or an `f64` returned by a function
    in an excluded crate); `clippy::float_arithmetic` uses type information
    and would catch these, and is not configured yet. An integer model,
    which a lexer cannot tell from other integer code; the activation rule
    above covers the one in the tree. Contract code: WASM guest float opcodes
    run in the consensus VM and are outside this invariant; they are made
    deterministic by NaN canonicalization with SIMD and relaxed SIMD off
    (`crates/vm/src/config.rs`). Number 35 is held by a proposal (account
    authority, PR #447) and is not reused.
