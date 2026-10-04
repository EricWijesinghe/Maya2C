# Session 2026-10-04 (overnight): explorer live, two consensus findings, validator path

Autonomous overnight session under Eric's standing approval
(2026-10-03: "everything is approved"; not economics, not mainnet launch,
not money).

## Live operations

- **explorer.maya2c.dev**
  - Before: Eric reported the explorer broken. The DNS name did not exist.
  - Now: `maya-explorer` runs on the testnet PC on 127.0.0.1:3000, and is
    routed by the Cloudflare tunnel (`cloudflared tunnel route dns`, ingress
    validated).
  - It is in `start-testnet.ps1`, so the five-minute watchdog restarts it.
  - Checked from outside:

    ```
    200 https://explorer.maya2c.dev/
    200 https://explorer.maya2c.dev/api/stats
    {"height":6003,"blocks_indexed":6004,"transactions_indexed":2,...}
    ```

  - CORS is `*` on the explorer and the gateway. The `/ws/blocks` upgrade
    answers `HTTP/1.1 101 Switching Protocols`. The site's live band
    therefore works.
- **Testnet upgraded to master `581f1122`** (#52: attested checkpoints,
  `--catch-up-from`, `get_bft_status`, epoch-log pruning) at 07:05 KST.
  - It used the same data directory, so no re-genesis. The previous
    binaries are in `bin/prev-20261004/`.
  - Height was 14,513 before the restart and 14,546 at the next read.
  - No errors in `node.log` since the restart.
- **Gateway fix deployed from PR #62 before merge.** It is stateless, and
  the old binary is kept. #52 had allowlisted `get_bft_status` and
  `get_checkpoint` but never dispatched them. Answers after the fix:

  ```
  get_bft_status: {"result":{"committed_round":726,"committee":1,"epoch":4,"follower":false,"round":728,"validator":0}}
  get_checkpoint: {"result":{"block":"ffc4f2dc…","epoch":4,"height":14763,"signatures":[[0,"bdaa79d6…
  ```
- **Live validator resource use** (measured 06:24 KST, 3 h 47 min after
  the re-genesis, 12,277 blocks):
  - working set 39 MB;
  - private memory 33 MB;
  - CPU 59 s;
  - data 150 MB.

- **Registration floor live at 10:03 KST.** The node runs master
  `864aa6d2`, and the process command line carries
  `min-register-bond 1000000000`. Afterwards `get_bft_status` returned
  `committed_round 4998`, committee 1, epoch 6. The previous binary is
  `bin/prev-20261004/maya2c-node.0705.exe`.
- **Release tagged.** `node-v0.1.0-testnet.1` on master `0269702e`.
  `release-binaries` run 37166896701 was queued at handover. The release
  notes were refreshed first (#63).

## Merged this session

#52, #53, #54, #55, #56, #57, #58, #60, #61, #62, #63, and #40 (rustfmt
fixed). #54 merged with its docs conflict resolved by hand. A local check
of the merged tree gave dag-bft 25 tests passed. #62 merged on 56 local
gateway tests passing, with CI's `nextest (core)` still pending. Master CI
re-runs both.

## Still open

- #59, ADR-040 part 1, waiting on CI.
- #21, Astro 5→7, needs a visual check.

## Findings

1. **Quorum 2f + 1 is unsafe off n = 3f + 1** (ADR-039, PR #54, fixed).
   - At n = 6, two disjoint halves of 3 certify.
   - At n = 2 and n = 3, every node certified alone.
   - The new quorum is n − f, which is identical at 1, 4 and 100.
   - `six_validators_split_in_half_halt_rather_than_fork` fails under the
     old formula.
2. **Committee capture** (ADR-039, open, mainnet gate 10).
   - One seat carries one vote and costs `min_self_bond`.
   - Absent seats beyond f halt the chain: one on today's testnet, two
     against four validators.
   - Nothing recovers a halted chain, because jailing happens only at an
     epoch boundary.
   - Mitigations:
     - #56 `--min-register-bond`, a testnet stopgap;
     - #59 stake-weighted engine (ADR-040 part 1);
     - ADR-040 part 2 needs Eric's decision.
3. **Weighted parent check refused horizon certificates** (found by both
   reviewers on #59, fixed before merge).
   - The bug would have broken ADR-038 catch-up.
   - It is pinned by
     `a_certificate_at_the_collection_horizon_is_accepted_without_its_parents`,
     which fails with the exemption removed (mutation-checked).
4. **No GitHub release has ever been published.** No tags exist, yet the
   site links to `releases/latest`. The `release-binaries` workflow has
   built all targets before (run 36478279508). The tag waits on master
   carrying #52 to #56.

## PRs opened

| PR | What |
|---|---|
| #53 | `l1-wallet generate` help said Ed25519; it makes hybrid ML-DSA-65 + SLH-DSA keys |
| #54 | quorum n − f, ADR-039, audit packet + KNOWN_ISSUES refreshed |
| #55 | `l1-wallet register-validator` / `delegate` |
| #56 | `--min-register-bond`; mainnet gate 10; chain-halt runbook for a committee without quorum |
| #57 | site: live validators + committed round from `get_bft_status` |
| #58 | "Run a validator" guide + application issue form |
| #59 | ADR-040 part 1: stake-weighted committee in the engine |
| #60 | NLnet draft refreshed to what ships |
| #61 | site: og:image card, twitter:image, theme-color, JSON-LD |
| #62 | gateway dispatches `get_bft_status` / `get_checkpoint`; test that every allowlisted method is dispatched |

Also pushed rustfmt to #40 (Maya Chat welcome bot, merged with master):
`cargo test -p maya-chat` gives 37 passed. The PR text said 38, and nobody
re-counted the difference.

## Command output (real)

```
cargo nextest run -p custom-l1-node -p maya-dag-bft -p maya2c-node        (#54)
  Summary [  83.069s] 1046 tests run: 1046 passed (1 slow), 2 skipped
cargo nextest run -p custom-l1-node -p maya-dag-bft -p maya-link-sim -p maya2c-node   (#59)
  Summary [  86.585s] 1064 tests run: 1064 passed (1 slow), 2 skipped
same, #59 merged locally with #52 (includes the 300 s rejoin test)
  Summary [ 255.626s] 1078 tests run: 1078 passed (2 slow), 2 skipped
cargo test -p maya-dag-bft --test auth_and_restart, exemption removed
  test result: FAILED. 6 passed; 1 failed
cargo test -p l1-wallet
  test result: ok. 19 passed; 0 failed
cargo test -p maya2c-node --bin maya2c-node
  test result: ok. 6 passed; 0 failed
npm run build (docs/site)
  70 page(s) built
```

The lint ratchet and the full workspace sweep were not run this session:
NOT VERIFIED. Each PR's clippy output for the files it touched was clean
or unchanged.

## Disk

`cargo xtask disk` reports build artifacts of 144.9 GiB, against a ceiling
of 30 GiB, with 169.7 GiB free. Do a `cargo clean` and clean the nested
target directories before the next long build.
