# 16 — Validator and key security

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 16

> DONE WHEN: remote signer works with the keystore backend and at least one
> HSM/KMS backend (or the ADR explains why none is available yet); all
> slashing-protection tests pass; DoS sim scenarios pass with honest finality
> inside SLO; incident response and emergency pause rehearsals are recorded here.

| Condition | Result |
|---|---|
| Remote signer, keystore backend | **built** (`crates/signer`, `bins/maya2c-signer`) |
| HSM/KMS backend | **not built.** ADR-022 explains why |
| Slashing-protection tests | **pass** (below) |
| DoS simulations, honest traffic inside SLO | **pass**, as a model (`[SIM]`) |
| Incident-response rehearsal | **rehearsed** (2026-09-27): stolen key → detected 250 ms → tombstoned 750 ms later → replacement seated 500 ms after, chain finalizing throughout (§5) |
| Emergency-pause rehearsal | **rehearsed** (2026-09-27): a security council now exists (2-of-3 in the rehearsal); a quorum closes one module in the block that carries its decision, transfers continue, lone/replayed/over-long actions are refused, the pause expires or is lifted (§6) |

## 1. Remote signer

```
$ cargo test -p maya-signer --profile ci
test a_crash_between_persist_and_signature_cannot_lead_to_a_double_sign ... ok
test a_torn_record_fails_closed ... ok
test hsm_and_kms_backends_say_why_they_are_unavailable ... ok
test a_node_restored_from_an_old_backup_cannot_resign_a_past_round ... ok
test signatures_verify_and_cover_the_domain_separated_message ... ok
test a_forged_frame_is_rejected ... ok
test a_replayed_frame_is_rejected ... ok
test unpinned_peers_are_refused_on_both_sides ... ok
test two_nodes_with_the_same_key_the_second_is_refused ... ok
test the_channel_authenticates_both_sides_and_carries_signing_requests ... ok
test keystore_round_trips_and_refuses_a_wrong_passphrase_or_a_tampered_file ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.66s
```

The three tests the brief names, mapped:

| Brief | Test |
|---|---|
| node restored from an old backup re-signs a past round → refused | `a_node_restored_from_an_old_backup_cannot_resign_a_past_round`: a conflicting re-sign is refused, and so is an unsigned round below the watermark; an identical retry is allowed |
| two nodes running the same key → second refused | `two_nodes_with_the_same_key_the_second_is_refused`: one signer and two nodes, then two signers linked by the interchange file |
| crash between sign and persist → no double sign | `a_crash_between_persist_and_signature_cannot_lead_to_a_double_sign`: the record is fsync'd *before* signing, so a crash leaves a record for a signature never released. `a_torn_record_fails_closed`: a half-written record makes the signer refuse to start |

The binary initialises a keystore and exports the interchange file:

```
$ maya2c-signer init --keystore k.json --passphrase-file pw     # prints the ML-DSA-65 public key
336ee8f0ad7bb0f1…
$ maya2c-signer export --keystore k.json --passphrase-file pw --protection p.jsonl
{ "metadata": { "interchange_format_version": "5-maya2c" }, …
```

Passphrases are read only from a file, never argv or environment values, and
are wiped after use.

## 2. Denial of service

`crates/dos-guard` puts a cheap check in front of every expensive one:
cookies, puzzles, per-IP/subnet budgets, per-peer cost accounting, mempool
admission policy, inbound diversity. It is **not wired into the node's
libp2p stack yet.**

```
$ cargo bench -p maya-dos-guard --bench handshake_gate
handshake gate, one core:
  reject (bad cookie)                   3887870 /s
  accept (cookie + ML-KEM encaps)         22255 /s
  ratio                                     175x
  puzzle  8 bits:      296 hashes avg,    0.04 ms avg to solve
  puzzle 12 bits:     5326 hashes avg,    0.79 ms avg to solve
  puzzle 16 bits:    53091 hashes avg,    7.89 ms avg to solve
```

```
$ cargo test -p maya-dos-guard --profile ci -- --nocapture
[SIM] spam 10x: unguarded honest p99 32500 ms (21840 admitted, 218160 spam verified); guarded p99 100 ms (48000 admitted, 510 spam verified)
[SIM] handshake flood (60 s, 5,000/s spoofed + 50 hosts in one /24): 1509 KEM operations, honest 1200/1200
[SIM] slow-loris: disconnected after 17500 ms; honest peer never throttled
[SIM] eclipse at 70% of attempts: attacker holds 9/50 connections (6 inbound, 3 outbound)
test result: ok. 4 passed; 0 failed …           (attack_sim)
test result: ok. 4 passed; 0 failed …           (units)
```

**What the simulations are.** A deterministic queue model with 100 ms ticks,
4 cores of verification per tick, and 1,000 µs per verification (measured).
The SLO line is admission within 1 s, half of the 2 s p50 finality target.
They show that the defences bound the attacker's cost to the node and keep
honest admission inside that line. They are not a consensus simulation:
"honest finality" is approximated by honest admission latency, because the
node has no BFT finality to measure (ADR-015).

**A bug found by the simulation.** The handshake flood underflowed a token
bucket: an address `x.y.z.0` is also the name of its own /24, and the two
budgets shared one bucket. Fixed with separate key spaces, and a unit test
was added.

## 3. Emergency pause

There is **no security-council pause** in `crates/governance`, and Master
Prompt 9 did not build one in this tree. The mechanism that exists is the
invariant guard (invariant 28). It checks conservation at every commit,
halts the offending module for 100 blocks, and keeps transfers working:

```
$ cargo test -p custom-l1-node --profile ci --test exploit_replays -- --test-threads 1
test draining_the_shielded_pool_trips_the_breaker_and_transfers_keep_working ... ok
test every_legitimate_subsystem_block_satisfies_conservation ... ok
… 11 passed; 0 failed; finished in 38.55s
```

Detection latency is **zero blocks**: the violating block itself trips the
breaker at commit. A human-initiated pause with timing measured is not
possible to rehearse until one exists.

## 4. Not done

- Sentry architecture. The node can dial `--bootnode`s, but it has no mode
  that stops it advertising its own address. The Ansible and k8s templates
  have no validator + 2-sentry topology.
- Host and supply chain:
  - hardened image beyond the read-only rootfs and dropped capabilities
    (`reports/10-launch.md`);
  - reproducible builds on two machines;
  - an offline release-key ceremony;
  - `cargo vet`.
- Incident-response rehearsal. `docs/security/INCIDENT_RESPONSE.md` and
  `docs/security/BUG_BOUNTY.md` exist; publishing the bounty needs
  "APPROVED: bug-bounty".
- Per-peer accounting and the admission policy in the node itself: the node's
  mempool has capacity and validity checks but no per-sender cap or
  fee-bump rule.

## 5. Incident-response rehearsal (added 2026-09-27)

Through the node's own DAG-BFT and staking code, four validators and one
spare on real RocksDB chains, timed in engine milliseconds:

```
$ cargo test -p custom-l1-node --test bft_staking_tests -- --nocapture
incident rehearsal (engine time): detected 250 ms after the stolen key signed; tombstoned 750 ms after detection; replacement seated 500 ms after that; chain kept finalizing throughout (height 9 and agreeing)
test incident_rehearsal_a_stolen_key_is_detected_slashed_and_replaced ... ok
```

The steps are the incident runbook's: detect (honest engines report the
second signed proposal as `Equivocation`), contain (anyone files the two
gossiped frames as `ReportEquivocation`; the key is tombstoned and half its
bond burned), recover (the operator registers a fresh key, seated at the next
epoch). What is not rehearsed: the human side — paging, communication, and
the time a person takes to notice.

## 6. Emergency pause — the security council (added 2026-09-27)

`state::council` + `TxKind::Council` (payload tag 51): genesis names M-of-N
ML-DSA-65 members; a quorum can pause **one module** for at most
`max_pause_blocks`, or resume it. The pause is a breaker record (reason
`CouncilPause`), so it can never stop transfers, staking or the council,
and it expires on its own. Approvals sign the council's nonce: a replay is
refused.

```
$ cargo test -p custom-l1-node --test council_pause_tests -- --nocapture
pause rehearsal: quorum decided at height 2 and the module closed in that block (0 blocks' latency); transfers landed throughout; resumed at 4 after 2 of the 10 paused blocks; a lone member, a replayed approval and an over-long pause were each refused
test result: ok. 2 passed; 0 failed
```
