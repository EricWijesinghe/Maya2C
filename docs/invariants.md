# Critical Invariants

Twenty-eight properties this tree must not lose, each with the reason it
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
   is a set someone can select by accident.
5. **`[profile.dev.package.*]` overrides are load-bearing, not tuning.** Without
   them `cargo test` reads as hung, not slow. Do not "clean them up".
6. **`dex` must stay dependency-free**, for the same reason as `ledger-math`.
   The visible cost is that it cannot hash, so pair and share-asset identifiers
   are derived in `custom-l1-node` and passed in as opaque bytes. That is the
   price of the boundary, not an oversight to tidy up.
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
20. **No ONNX runtime on the consensus path.** The node depends on `maya-zkml`,
    which is the verifier only; `tract-onnx` lives in `maya-zkml-prover`, which
    the node takes as a dev-dependency and nothing more. A separate crate rather
    than a feature, because a feature can be switched on by any crate in the
    graph through unification and a crate the node does not depend on cannot.
    Most ONNX models are floating point, and a float in a consensus rule is a rounding
    mode two validators can disagree on. Verification checks a proof; nothing
    in a block runs a model. See `docs/zkml.md`.
21. **A host function that does native work charges fuel for it, first.** Gas
    is wasmtime fuel and cannot see native work, so `host_verify_zkml_proof`
    charges a *measured* price (`crates/vm/src/zkml.rs`, calibrated by
    `crates/vm/tests/fuel_calibration_tests.rs` and `crates/zkml-prover/benches/verify.rs`) before
    it reads a byte, and traps out-of-fuel before the verifier runs. There is
    deliberately no tensor host function: guest wasm is priced exactly by the
    fuel meter, and a hand-set per-MAC price would be consensus-critical and
    wrong on some machine.
22. **zkML stays dark until its SRS is real and gas is capped.** The SRS in
    `crates/zkml/src/srs.rs` is derived from a public seed, so anyone can forge proofs;
    and no cap bounds a call's `gas_limit`, so a fuel price bounds nothing
    absolutely. `ZKML_ACTIVATION_HEIGHT` is `u64::MAX`, and
    `state::zkml::check_setup` refuses mainnet the moment it is anything else
    while `SRS_IS_TRUSTED` is false.
23. **Every constraint in `crates/zkml/src/circuit.rs` has a test that fails without
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
    - `LOCAL_ONLY_PREFIXES` (`undo:`, `blk:`).

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
    [docs/invariant-guard.md](docs/invariant-guard.md).
