# Shielded pool: activation plan after genesis (research, 2026-11-10)

Verdict: the pool is a real, post-quantum-plausible STARK (Plonky3 FRI, Poseidon2 +
Keccak, no pairings, no trusted setup), but it is not ready to switch on. Four
blockers come first.

Decision record: [ADR-087](../adr/ADR-087-shielded-pool-activation-after-genesis.md).

## Facts (file:line, verified 2026-11-10)

The first draft was written from a read-only pass. Each row below was re-checked against
the source; corrections are marked **[corrected]**.

| Piece | Location | Fact |
|---|---|---|
| Joinsplit AIR 2-in 2-out | `crates/zk-stark/src/pool/joinsplit.rs:53` | `ROWS = 128`. features.toml said "2^8 rows" **[corrected: the drift text is at features.toml:1845, not 1837; now reads 128 rows]** |
| Value conservation | `joinsplit.rs:355-401` | limb-wise, 3-bit carry, radix 2^27 (`LIMB_BITS = 27`); doc comment at 28-32 |
| Dummy inputs | `joinsplit.rs:371-376` | dummy * value = 0 (checked on both limbs) |
| Public amounts | `pool/witness.rs:76-84` | range-checked to 2^54 - 1 in Rust, not in the AIR (audit assumption) |
| Note cap | `pool/note.rs:26-30` | `LIMB_BITS = 27`, `VALUE_BITS = 54`, `MAX_VALUE = 2^54 - 1` |
| Hash | `crates/zk-stark/src/hash.rs:1,31` | Poseidon2 over BabyBear, `WIDTH = 16` |
| PCS | `crates/zk-stark/src/config.rs:50-54` | `LOG_BLOWUP = 2` (blowup 4), `NUM_QUERIES = 100`, `QUERY_POW_BITS = 16` |
| Security | `crates/zk-stark/src/proof.rs`, features.toml:1845 | "113 bits conjectured, 99 proven"; proof ~383 KB (note text) |
| Tree | `pool/tree.rs:18-21` | `TREE_DEPTH = 32` |
| Dependency gate | `crates/zk-stark/tests/pqc_zk_tests.rs` | forbids halo2, Groth16, BN254, BLS12-381 (list begins near line 22) |
| Dependency pins | `crates/zk-stark/Cargo.toml:14-28` | all `p3-*` pinned `=0.7.0` |
| Audit flag | `pool/mod.rs:36`, `:43-45` | `CIRCUIT_IS_AUDITED = false`; `circuit_is_audited()` returns it |
| Node apply | `crates/node/src/state/shielded.rs` | duplicate nullifier, proof, anchor window, nullifiers, turnstile settle, 16 per block cap |
| Gate | `crates/node/src/state/context.rs` (`shielded_activation`), `consensus/chain.rs:349-355` | `Chain::context_at` passes `config.shielded_activation` through `with_shielded_activation` (line 352) |
| Genesis commitment | `crates/node/src/genesis.rs:1105-1112` | **[corrected: this is the genesis-id commitment for a set activation height, not a check; it leaves the base id unchanged when the height is None]** |
| Wire payload | `crates/node/src/core/payload.rs` | no ciphertext or envelope for notes |
| Envelope | `core/transaction.rs` | public key + signature + nonce: sender public |
| Conservation guard | `state/invariant_guard/conservation.rs` | pool balance vs public flows only |
| Drain breaker | `anomaly.rs:181-200`, `limits.rs` | `SHIELDED_DRAIN_BPS = 5000`, floor 1,000,000 |
| Mainnet guard | `bins/maya2c-node/src/main.rs:117-135` (call at 855) | **[corrected: the guard function starts at 117; the test-pinned condition is at 124]** refuses a value-bearing network with an unaudited pool |
| Upgrade features | `crates/node/src/upgrade.rs:34` (`SUPPORTED_PROTOCOL_VERSION = 2`), `:36-60` (`FeatureSet`, `KNOWN = (1 << 8) - 1`), `:195-214` (`features()`) | confirmed; no SHIELDED bit; `FeatureActivations` (line 81) has no `shielded` field |
| Lead constant | `crates/governance/src/limits.rs:90` | `MIN_UPGRADE_LEAD_BLOCKS = 2_592_000` |
| Viewing keys | `crates/privacy/src/lib.rs` | ML-KEM-768 envelopes, RESEARCH, not wired |
| Coin decimals | `crates/amount/src/lib.rs:52-54` | `COIN_DECIMALS = 24` (ADR-068) |

## Blockers

1. **Cap is 2^54 base units, not 2^58 (pool).** At 24 decimals that is about 1.8e-8 coin
   per note. The 2^58 figure is the private-state ceiling (`private_state.rs:36-38`,
   two 29-bit limbs), which is about 2.9e-7 coin at 24 decimals. ADR-067 said 2^58 for the
   pool and 0.29 of a coin at 18 decimals; both figures were wrong for the pool. ADR-067
   and ADR-066 are corrected in this change. The repeated figure in `docs/privacy/LIMITS.md:35`
   and `reports/27-privacy.md:64` describes private fields and is correct as written.
2. **No schedule path.** `FeatureSet` bits 0-7, `KNOWN = (1 << 8) - 1`
   (`crates/node/src/upgrade.rs:36-60`), no SHIELDED bit; `features()` (`upgrade.rs:195-214`)
   does not feed a shielded activation.
3. **Sender is public** (signed account + nonce on every shielded tx).
4. **No note delivery** (no ciphertext in the payload).
5. features.toml drift: "2^8 rows" (now 128); "recursive Halo2 ZK-STARK state folding" and
   "Binius/Halo2" text. Halo2 is not in the stack (ADR-008). Fixed in this change.
6. Plonky3 pinned `=0.7.0`; upstream at v0.9.0-rc.2 (2026-10-16). A bump changes the proof
   format.

## Supply-integrity hole

The turnstile tracks public flows only. A soundness bug that mints notes inside the pool
lets an attacker withdraw up to the pool balance (other users' value) without breaking the
turnstile. Only the pool cap (not built) and the drain breaker bound the loss. Needed: a
sum-of-unspent-values check not derivable from public flows (committed per-note total or
in-circuit minted accumulator). New invariant number only once a test pins it.

## Activation through ADR-082

1. `FeatureSet::SHIELDED = 1 << 8`, `KNOWN = (1 << 9) - 1` (append-only). Bit 9 is reserved
   for ACCOUNT_AUTHORITY (wallet-v2, ADR-086).
2. `shielded: u64` in `FeatureActivations`, filled in `features()`.
3. `SUPPORTED_PROTOCOL_VERSION` 2 to 3 in the audited binary only.
4. Precedence: genesis height wins if set; if genesis is SHIELDED_NEVER, the schedule supplies
   it; both set, refuse at startup (`Chain::context_at`, `consensus/chain.rs:349`; genesis
   commitment `genesis.rs:1105`).
5. Governance writes `up:v3` with at least `MIN_UPGRADE_LEAD_BLOCKS` (2,592,000 blocks, about
   30 days) lead.
6. `CIRCUIT_IS_AUDITED = true` only in a separate reviewed commit; the `main.rs` guard reads
   the effective schedule.
7. v2 validators halt at H.
8. Amend ADR-082 "no funds, no minting": a value-moving feature needs an audit record in the
   repo and `circuit_is_audited() == true`, enforced in the writer.

## Privacy gap vs Monero / Zcash

| Property | Zcash Orchard | Monero | Maya2C today |
|---|---|---|---|
| Sender hidden | yes | yes (ring) | no |
| Recipient discoverable | yes | yes | no |
| Amounts hidden | yes | yes | in pool; boundary public |
| PQ | no | no | plausibly yes (hash-based) |
| Trusted setup | no | no | no |

Mitigations to design: one-time signing key per shielded tx with account-free replay
protection; stem-phase relay (no dandelion in `crates/node/src` today); sealed mempool
(`crates/node/src/sealed/mod.rs`) hides payloads not sender; denominations for boundary
amounts; ML-KEM note ciphertexts via `crates/privacy` (new payload version). No backdoor, no
mandatory viewing key.

## PQ alternatives (eprint titles fetched 2026-11-10)

Keep Plonky3 STARK (recommended). References: Circle STARKs 2024/278, ethSTARK 2021/582,
Binius 2023/1784, Poseidon 2019/458, Poseidon2 2023/323, LaBRADOR 2022/1341, Greyhound
2024/1293, LatticeFold 2024/257, MatRiCT 2019/1287, MatRiCT+ 2021/545, ZK for STARKs
2024/1037, QROM Fiat-Shamir 2019/190; Zcash ZIP 224; Monero FCMP++ (kayabaNerve, last push
2025-08-26). Target at least 128 proven bits (today 99).

## Phased rollout

- P0 (pre-genesis or dark): correct ADR-067 and features.toml (done here); decide cap width;
  sender-hiding design; SHIELDED bit and precedence behind SHIELDED_NEVER
  (`postgen/shielded-feature-bit`); dark tests.
- P1 testnet: functional tests (testnet already runs the pool), viewing-key envelopes behind a
  new payload version, time-boxed.
- P2 audit (after mainnet per owner directive; hard gate; never call self-review independent).
- P3 mainnet capped pool via `up:v3`, at least 30 days out; caps MISSING (Eric decides).
- P4 lift cap / 256-bit widening, each with its own ADR and audit scope.

## Red-team checklist

Wrapped limb above cap; carry trick with raw public limb at or above 2^27; dummy with value;
fork anchor after reorg; two joinsplits sharing a nullifier; replay under another signer;
proof reuse with different recipient (public values in transcript?); mint-then-drain under cap;
FRI grinding / weak Poseidon2 rounds; QROM; signer/nonce linkage; gossip origin linkage;
boundary amount matching; decode bombs; genesis plus schedule double enable; audited flag on a
different AIR.

## Decisions for Eric

Cap width (keep 2^54 for capped phase vs widen before audit, decide before the audit);
pool total and per-tx caps (MISSING); sender-hiding design; auditor and cost (after mainnet).
