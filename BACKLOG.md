# Backlog

Work found during a session that is bigger than the session. Each item
names its priority band (CLAUDE.md, Operating Protocol, "Choosing what to
do next") and where it came from. Take from the top of the highest open
band; move an item to PROGRESS.md when it starts.

## P1 — mainnet launch gates (docs/mainnet-v1-plan.md)

- [x] ADR-068 step 2: 24 decimals (feat/decimals-24) (wallet DECIMALS, SDKs, explorer, governance COIN, pool, genesis amounts).
- [x] ADR-068 step 4 (clean, DECISIONS 10-28): audit consensus amount × amount products for 512-bit or checked math.
- [x] spec: MINE-* rules + vectors (spec/09-mining.md; node conformance test not yet run) for the mining schedule incl. the tail (ADR-044 shipped without spec).

- **Gate 8: historical catch-up for a restarted validator.** The engine
  answers `Fetch` only from memory (`GC_DEPTH` = 50 rounds), so a validator
  down longer than about 30 s is stranded for good. Serve fetches below the
  horizon from `certs.log` was built and measured: peers sent the right
  certificates but gossip lost the replies, and healthy validators dropped
  frames under the load. Snapshot bootstrap lands outside the window too.
  Plan: ADR-038 (quorum-attested checkpoints, import blocks up to one with
  re-execution). Source: four-validator dry run, 2026-10-04.
- **Gate 10: more than f validators that each need to follow.** Each was
  down past the engine window. Followers vote but cannot attest, so the
  builders alone never reach the n − f checkpoint quorum, and the followers
  never import. Eric kept n − f (2026-10-05), so the fix cannot lower it.
  Candidate: blocks commit to the ordered frontier, letting a follower
  derive from its own tip. Design in ADR-041 (proposed); needs Eric's
  approval, since it changes the block format. Today's recovery is a
  snapshot restore.
- **Major dependency bumps, migrated by hand**: hmac 0.13, rand_chacha 0.10
  (crypto, so reviewed), and Plonky3 p3-* 0.8 (zk-stark). Dependabot skips
  majors (dependabot.yml). Closed PRs #91–#94 show the build breaks.
- **Gate 9: compact consensus logs within an epoch** for a chain without
  staking (epoch 0 for ever). `safety.log` keeps only rounds the engine could
  still sign; `certs.log` only rounds at or above the horizon. Rewrite then
  atomic rename. Source: same.

- **Wallets pin the genesis of each known network** and show the network
  name and a short fingerprint before signing; the Ledger app names known
  geneses. Today a malicious node can make a wallet sign for another chain
  (ADR-036, "Open before mainnet"). Source: security review, 2026-10-03.
- **Key the kept block preview on wire hashes, not txids**
  (`crates/node/src/state/preview.rs`), so a multisig body re-encoded with
  other approvals cannot reuse a preview. Predates ADR-036. Source: same.
- **Ceremony runbook checks the mainnet genesis id differs from every
  published testnet id** (gate 5). Source: same.
- **htlc-watcher and pool-service fetch the genesis per transaction**;
  cache it once per connection, as the faucet does. Source: rust review,
  2026-10-03.

## P2 — make claimed-working features verified

- **Joining nodes dial private addresses learned from the DHT.** A helper's
  node behind only the wss bootnode tried 172.25.x, 192.168.64.x and 10.1.x
  (WSL, the stagenet containers, attacknet on the seed's PC), all refused, and
  logged each as `outgoing connection failure #n`. Filter non-routable
  Kademlia addresses unless the node itself is on that network, or stop the
  seed advertising them. Harmless, but it reads as broken. (maya2c-c9's
  real-PC join, 2026-10-13: testnet.3 bootstrap to ready in ~13 min.)
- **Catch-up calls an already-held block "malformed".** v4's log (2026-10-13)
  has 1,975 lines `catch-up: malformed wire encoding: a block did not extend
  the tip: Duplicate {..}` while it follows attested blocks. `InsertOutcome::
  Duplicate` should be skipped quietly; logging it as malformed hides real
  decode errors in noise. Reported by maya2c-c9.
- **HTLC two-devnet swap times out under full parallel load** (1,058 s in a
  workspace nextest run; 18 s alone). Mark it heavy / serial in nextest.
- **Chat envelopes are twice the size they need to be.** `Envelope::encode`
  (ciborium) writes ciphertext `Vec<u8>` as a CBOR integer array, about two
  bytes per byte: a 120 KiB sticker seals to 247,427 B. `serde_bytes` on the
  ciphertext fields would halve every message and double the sticker limit,
  but it changes the chat wire format for the relay and every client: do it
  with a version byte, while users are few.

- **Measure line coverage of the core crates** with `cargo llvm-cov`
  (installed, 0.8.7; never run). M1 criterion 8. Source: EMPIRE.md.
- **Property tests for fees, consensus and VM.** State has a seeded
  property test (`crates/node/tests/supply_property_tests.rs`); a search
  found none for `fee-market`, `dag-bft` or `vm`. First confirm by reading
  their test suites, then add seeded property tests. M1 criterion 5.
- **Gap-register allowlist.** `scripts/gap_register.py` counts reviewed
  guards as P0 (DECISIONS.md, 2026-09-28). Add a reviewed-guard marker so
  the count means open work. Port to Rust (`cargo xtask gaps`) at the same
  time: new tooling here is Rust.
- **Re-check that every number in reports 12, 13, 14, 26** carries commit,
  hardware, OS and command (Production Standing Orders). M2.
- **PROGRESS.md is stale in places**: "Phase B — physical layout — not
  started" although the tree already has `crates/ bins/ apps/ hal/`; the
  section header still names branch `claude/task-0g86kl`, which was merged
  and deleted. Reconcile against git history.

- **`xtask sweep` should record a dirty tree.** The record names only
  `git rev-parse HEAD`; the 2026-09-28 re-run ran with uncommitted fixes on
  top. Add `git status --porcelain` count to `Sweep`, and have `status`
  print it.
- **doc-coverage takes about 35 minutes** (2,094 s on 2026-09-28) and runs
  only nightly; a broken intra-doc link blocked it unseen. Add
  `-D rustdoc::broken-intra-doc-links` to a fast per-PR `cargo doc` job.

## P3 — current milestone (M1)

- Decide MP06's scope for M1 (EMPIRE.md, Q1) — Eric.
- `cargo xtask bench` with commit/hardware/OS/command stamped into every
  result (CLAUDE.md says it does not exist).

## P4 — adoption-critical

- [ ] **Eric 2026-10-28, starts once everything is at zero errors** (CI green, suites passing, mainnet stack merged): the biggest GUI/UI upgrade, with fully deployable, signed applications on every platform: wallet, chat, hub/miner, explorer, site. Windows, Linux, Android now; macOS/iOS after the MacBook. Never flagged by OS, antivirus or stores.

- MP23: move the safe-contract resource rules from opt-in imports onto the
  consensus import surface (needs an ADR and invariant work).
- MP24: guest traps mapped to source lines (needs a backtrace out of the
  consensus execution path without changing results).
- MP22/MP29/MP24 usability and developer studies — need participants (Eric).

## P5 — differentiators

Eric 2026-10-29: build all of these after mainnet (each needs a dated `docs/prior-art/` search before any "first" claim, and an ADR):
- [ ] **Decentralization-aware rewards**: per-epoch Nakamoto coefficient / Gini from `staking/concentration.rs`; rewards tilt toward small validators and miners when control concentrates.
- [ ] **Cryptographic sunset schedule**: a suite deprecation announces a height, a migration window, wallet warnings and an automatic move to the successor suite (`crypto-pq/agility`, invariant 29).
- [ ] **Post-quantum time-locked messages and payments**: unreadable and unspendable until a future height, ML-KEM sealed (sealed mempool / threshold encryption). Prior art: drand tlock (not PQ, not native).
- [ ] **AI-work mining**: miners earn for verified AI inference via the ADR-044 WorkClaim kind. Prior art: Bittensor, Gensyn.
- [ ] **Offline payments**: PQ-signed transfers carried by Bluetooth, LoRa or satellite and settled when a relay is online (chat DTN design).
- [ ] **Quantum Harbor safe-deposit**: BTC/ETH holders commit a PQ proof of ownership now, claimable if their keys are broken later (`quantum-harbor`).
- [ ] **Duress keys** (Scam Shield): a registered panic key; a transfer signed with it sends a decoy amount, locks the real balance 72 h and alerts guardians.
- [ ] **Safety-window payments** (Scam Shield): opt-in protected transfers, spendable after a window, recallable with guardian-approved fraud proof; `threat-intel` flags known scam addresses.
- [ ] **On-chain AI judge**: contracts name an open model by hash; disputes resolved by deterministic integer inference every validator reproduces (builds on fee-market `MODEL_V1`).
- [ ] **Living inheritance**: owner heartbeat; missed check-ins, then a guardian veto window, then transfer to heirs.
- [ ] **Phone-mined first fees**: a zero-balance user's visible, opt-in proof of work counts as a mining share that pays its first fees (ADR-044 share lane; no hidden mining).

- MP25: zero-knowledge light client (research-sized).
- MP28: outside-asset route into PQ vaults (depends on MP25); archive task
  sealing.
- Maya Chat difference: dated searches for PQ authentication and
  chain-account identity (docs/prior-art/p2p-chat.md).

## P6 — deferred and frontier (untouched until their milestone)

- **Next evolution brief (Eric, 2026-10-30), after mainnet.** The full feature list is in
  [docs/master-prompts/next-evolution.md](docs/master-prompts/next-evolution.md): privacy by default,
  sharding, AI agents, identity, interop, SDKs in five languages, mobile and browser validators.
  First step: a gap table against `features.toml`. Several items already exist (DAG-BFT finality,
  PQ crypto, WASM VM, Rust SDK). Items that collide with an invariant are reported before any ADR;
  homomorphic encryption and ring signatures, for example, need a PQ variant (invariant 2).

- MP06: dealer-free MPC offline phase; per-order dark-pool proofs;
  proof-of-personhood liveness and uniqueness (needs attested hardware,
  invariant 11); Landsat second oracle sensor (requester-pays bucket).
- MP24: lazy fork state — conflicts with invariant 24; not to be built as
  specified.

- **`--min-epoch` signing floor (ADR-043 follow-up, security review 2026-10-10).**
  A validator restarted with an empty safety log is protected only by the
  operator runbook check (`docs/runbooks/chain-halt.md`). A node-level floor
  that refuses to sign below a given epoch would make it mechanical.
- **Catch-up source agreement (security review 2026-10-10).**
  `within_engine_window` trusts one peer's tip. Require two sources to agree
  before skipping follower mode, and rotate sources in `follow_loop`.
- **Snapshot RPC bandwidth (security review 2026-10-10).** Batches pass the
  per-IP limiter unmetered (`rpc/server.rs`). Meter them, and cap the
  snapshot bytes served per client.
