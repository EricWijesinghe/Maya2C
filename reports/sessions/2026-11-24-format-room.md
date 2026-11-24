# 2026-11-24 — Format room before genesis (ADR-155)

Lane: format room (Eric 2026-11-24: every limit liftable without a hard fork,
each fix a guarantee from block 0). Four pull requests, stacked where an open
PR already owns the code. Every command ran through `cargo-slot.sh` on one
Windows 11 host; outputs below are copied from the runs.

## Pull requests

| PR | Branch | Base | Items |
|---|---|---|---|
| #696 | `feat/format-room-1124` | `master` | NEW-1 vertex batch root and header-sized evidence (CON-18); C2/NEW-7 certificate version byte and four-byte ids (CON-19); P7 account auth kind (ROOT-1); P3 STARK proof version byte |
| #700 | `feat/format-room-upgrade-record` | #506 | G21/NEW-3 release hash, 128-bit feature set, versioned `up:v` record (CON-14) |
| #702 | `feat/format-room-block-envelope` | #687 | B1/NEW-4 `block.max_bytes` (tag 21) and `block.max_transactions` (tag 22) under compiled ceilings; derived epoch sync bound; 2 MiB contract decoder (CON-12, CON-13, CON-17, invariant 33) |
| #705 | `feat/format-room-header-slot` | #573 | C12/B5/B6 header extension root (177-byte header, CON-1); B17 wire version 14 reserved (ENC-1) |

## Red before green

```
#696  cargo test -p custom-l1-node --test format_room_tests       FAILED. 0 passed; 7 failed
      evidence of 8415023 bytes does not fit a 4194304-byte transaction
      cargo test -p maya-zk-stark --release --test gadget_tests a_proof_names
      version 0: Err(Malformed("Hit the end of buffer, expected more data"))   FAILED. 0 passed; 1 failed
#700  cargo test -p custom-l1-node --test upgrade_record_tests    did not compile (no binary_hash; 1 << 100 overflows a u64 set)
#702  cargo test -p custom-l1-node --test block_envelope_tests    did not compile (no keys, ceilings or max_epoch_bytes)
      runtime red on the old code: Decode("byte length 2097152 exceeds maximum 524288"),
      Decode("collection length 65537 exceeds maximum 65536")              FAILED. 0 passed; 2 failed
#705  cargo test -p custom-l1-node --test header_slot_tests       FAILED. 0 passed; 3 failed (left: 145 right: 177)
```

## Green

```
#696  node lib 543 passed; format_room 7; consensus_conformance 1; conformance 6; genesis 23;
      cert_rpc 7; stateless_equivalence 11; bft_staking 8; bft_follower 13; bft_node 16;
      hash_migration_rehearsal 2; dag-bft, spec-ref, stateless-core all ok; zk-stark gadget 15, pqc 4;
      spec-coverage --strict: 64 rules, 169 vectors; 61 consensus rules covered, 0 gaps;
      cargo check -p maya2c-node -p l1-wallet -p maya-light-client --tests: Finished
#700  node lib 541; upgrade_record 4; upgrade_schedule 11; unknown_bit_halt 1; upgrade_rehearsal 4;
      genesis 23; governance 25; bft_node 17; council_pause 5
#702  governance lib 47, fee-market all ok; node lib 556; block_envelope 7; block_bytes 10; payload 29;
      governance 34; consensus_conformance 1; bft_follower 13; pruned_node 19; bft_node 16; cert_rpc 7;
      cargo check -p maya2c-node: Finished
#705  node lib 542; header_slot 3; conformance 6; consensus_conformance 1; crypto 31; genesis 23;
      signed_header 5; payload 29; block_bytes 10; bft_node 16; stratum-v2 and spec-ref ok;
      spec-coverage --strict: 64 rules, 167 vectors; 61 consensus rules covered, 0 gaps;
      cargo check -p maya2c-node: Finished
```

## NOT VERIFIED

- `cargo xtask coverage --verify-targets`: left to CI (agent rules).
- `hal/cuda-miner` (CUDA, outside the workspace): header length and parity KAT edited, not built.
- `fuzz/` (nested workspace): `account_decode`, `header_decode` edited, not built.
- `maya-light-client` and `maya-pool-service` tests on #705: two runs stopped at
  `could not compile web-sys (lib)`, a dependency the change does not touch.
- CI on all four PRs was still queued when this report was written (49 runs queued, 1 running).

## Not done, with reasons

- Chunked gossip and sync framing: node-local; #702 asserts the highest votable
  block fits one gossip frame and one sync response.
- x2 ramp per activation of `block.max_bytes`; per-vertex proposal cap and fee
  target not re-derived from a raised cap.
- Block attestation frame keeps a two-byte validator index (attestation evidence is #680/#686's).
- G17 reserved cancel kind for upgrades.

## Resume

Merge order: #696 (master); #506 then #700; #687 then #702; #573 then #705.
The ADR-155 file is identical on all four branches; `DECISIONS.md` entries
append at the end of each base and may need a trivial merge.
