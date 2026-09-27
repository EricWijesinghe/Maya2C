# 17 — The integration layer

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 17

> DONE WHEN: mesh-cli checks pass; the deposit service passes 100,000
> deposits with restarts; the offline signing loop and WalletConnect flow pass
> e2e; the 5-minute developer path is measured on three OSes; every guide's
> commands run in CI; reports/17-integrations.md is complete.

| Condition | Result |
|---|---|
| mesh-cli `check:data` / `check:construction` | **`check:data` passes** (2026-09-27, §6): 5/5, 410 reconciliations, 0 failed. **`check:construction` cannot run**: Mesh defines no post-quantum curve or signature type |
| Deposit service, 100,000 deposits with restarts | **passes**: 100,090 deposits, 40 SIGKILLs |
| Offline signing loop, e2e | **passes** |
| WalletConnect flow, e2e | **not done.** WalletConnect v2 needs a relay project id from WalletConnect Cloud — an account the owner registers |
| 5-minute developer path on three OSes | **Linux only**, partly (§5) |
| Every guide's commands in CI | **not done** |

## 1. Deposit service

`crates/deposit-watcher`. It keeps an append-only journal with one line per
finalized block, carrying the cursor and that block's credits together, and
fsyncs before moving on. A torn last line is cut off and its block
reprocessed; txids are remembered against re-delivery.

```
$ cargo test -p maya-deposit-watcher --profile ci -- --nocapture
test a_torn_final_line_is_discarded_and_the_block_reprocessed ... ok
100090 deposits over 10010 blocks; 40 SIGKILLs mid-run; credited 100090; ledger matches ground truth: true
test a_hundred_thousand_deposits_across_kill_nine_restarts_credit_exactly_once ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.27s
```

The child process is killed with SIGKILL, with no destructors and no
flush. The synthetic chain includes 10 re-delivered txids, which must not
be double-credited, and 2 non-customer transfers in every 12. The first run
of this test carried 99,990 deposits, 10 short of the brief's 100,000, so the
block count was raised rather than the requirement read loosely.

It reads from a `BlockSource` trait. The test implements it with a generator;
a node-RPC implementation is not written.

## 2. Offline signing

```
$ cargo test -p custom-l1-node --profile ci --test offline_signing
test a_frame_altered_after_signing_is_refused ... ok
test build_export_sign_offline_and_broadcast ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.07s
```

The loop: an online side with no key builds an unsigned v5 frame, and an
offline side decodes it, shows the operator what it is signing, and signs.
The signed bytes are admitted by a real `Mempool` and applied by a real
`StateDB`. A frame whose amount changes on the way back is refused.

## 3. Documents

- `docs/integrations/EXCHANGES.md`:
  - finality in numbers. Under today's PoW it is probabilistic, and the
    Nakamoto table gives, for example, 24 confirmations (6 min) at a 30 %
    attacker for P < 10⁻³;
  - one address per customer, and no memo field exists;
  - native withdrawal batching;
  - the fee market is inactive;
  - what to do in a halt.
- `docs/integrations/CUSTODY.md`:
  - HSMs must hold *both* halves of the hybrid;
  - elliptic-curve MPC does not apply to ML-DSA or SLH-DSA, so on-chain
    multisig (wire v8, active since genesis per ADR-013) is the supported path;
  - the air-gapped flow.
- `chain/maya2c-testnet.json`:
  - listing metadata with **empty** RPC and explorer lists, because none
    exist;
  - **no** decimals and **no** symbol, because the tree defines neither.
    Inventing either for a listing would be the mistake
    `crates/node/src/rpc/market.rs` warns about.
- ADR-023: EVM is not in v1. When it comes, EVM accounts are labelled
  classical security, and the post-quantum path is account abstraction with
  an ML-DSA precompile.
- ADR-024: oracles use the native module (median, height-based staleness,
  optional), with a named authority set because staking does not exist.

## 4. Not done

- Mesh (Rosetta) Data and Construction APIs, and mesh-cli.
- WalletConnect / Reown, a browser-extension provider API, and a hardware
  wallet e2e with the GUI. The Ledger app exists under `apps/`, but its tests
  need the device or Speculos, neither run here (`reports/04-completion.md`).
- A GraphQL indexer and a subgraph template.
- A contract-verification service (source + compiler → reproducible Wasm
  hash).
- A docs site with CI that runs every guide command.

## 5. Developer path, measured

There is no `maya2c init` command. The closest measured path is the
local-docker devnet, on Linux (4 vCPU, `reports/12-baseline.md`):

- with a built binary, 12 nodes up and answering RPC in 24 s
  (`reports/10-launch.md`); one node, the same command with `NODES=1`;
- a cold `cargo build --workspace`, 4 m 54 s as measured in `CLAUDE.md`
  (2026-09-20), not re-measured here.

So "under 5 minutes on a fresh machine" is **borderline on Linux** and
**unmeasured on macOS and Windows**. No template contract, deploy or verify
step exists.

## 6. Mesh Data API (2026-09-27, Windows workstation)

`crates/mesh-api` (`maya2c-mesh`) serves `/network/{list,options,status}`,
`/block`, `/account/balance` (current and historical) and `/mempool` over a
node's JSON-RPC. The node gained three reads for it:

- `get_balance_changes(height)`: every balance the block moved, before and
  after. It is written in the same batch as the undo journal from the
  journal's prior values and the overlay's new ones. It is deleted on revert
  and pruned with the body (`state::balance_changes`, local-only under
  invariant 25).
- `get_account_at_tip(address)`: balance and tip read under one lock.
- `get_balance_at_height(address, height)`: the current balance with each
  later block's recorded change undone, bounded to 100,000 blocks back.

Each block is served as its signed transactions (no operations) plus one
`block-balance-changes:<id>` transaction with a `BALANCE_CHANGE` per moved
balance. Fees, burns and staking payouts move balances without an output
saying so. Taking the changes from the node's before/after record is what
lets a reconciler's sums close.

```
$ cargo xtask mesh-check          # mesh-cli v0.10.4, one-validator DAG-BFT devnet
sent 23 transfers
Success: Reconciliation Coverage End Condition [Coverage: 100.000000]
| Request/Response   | Rosetta implementation         | PASSED |
| Response Assertion | All responses are correctly    | PASSED |
| Block Syncing      | Blocks are connected into a    | PASSED |
| Balance Tracking   | Account balances did not go    | PASSED |
| Reconciliation     | No balance discrepancies were  | PASSED |
| Blocks                   | # of blocks synced             |          46 |
| Operations               | # of operations processed      |          88 |
| Accounts                 | # of accounts seen             |          23 |
| Active Reconciliations   | # of reconciliations performed |          88 |
| Inactive Reconciliations | # of reconciliations performed |         322 |
| Failed Reconciliations   | # of reconciliation failures   |           0 |
mesh-check: passed
```

**What getting there found:**

- **Tip-only reconciliation never finished.** With historical lookup off,
  mesh-cli reconciles only at the tip. A devnet producing blocks as fast as
  mesh-cli syncs them left it at 56 of 824 blocks after 900 s, with no
  error. Historical lookup (`get_balance_at_height`) fixed it.
- **An upstream mesh-cli bug.** In v0.10.4 the `reconciliation_coverage.index`
  end condition is inverted: `if *Index < blockIndex { continue }` makes it a
  maximum rather than the documented minimum. The check uses `account_count`
  instead. It has not been reported upstream; that is an outward-facing step
  for the owner.
- **Windows paths.** mesh-cli splits the config file's directory on `/` only,
  so a mixed `D:/…\mesh-config.json` resolved relative paths against the
  wrong directory. The driver passes the config by its bare name.

**Construction API: not implemented, and not checkable as specified.**
`check:construction` signs with keys mesh-cli generates itself, from
`CurveType` / `SignatureType` enumerations that contain no ML-DSA or hybrid
scheme. Passing it needs a Mesh specification extension, a decision outside
this repository. Offline signing (§2, and `l1-wallet send --no-broadcast`) covers
the custodian flow the Construction API would serve.

**Review (rust-reviewer + security-reviewer), and what changed:**

- **CRITICAL, fixed.** The first `get_balance_at_height` walked up to
  100,000 blocks while holding the chain mutex that block production takes.
  One caller could stall fork choice. The walk now runs outside the lock; the
  lock is retaken only to confirm the tip is still canonical, so a reorg
  mid-walk is refused. The bound is now `PRUNE_DEPTH` (30,000), the depth the
  changes are actually kept for. `mesh-check` re-run after the fix: 72
  blocks, 710 reconciliations, 0 failed.
- **MEDIUM, fixed.** Storage faults in the new methods used the "rejected"
  code (-32000) with the RocksDB text. They are now -32603 with a fixed
  message, detail to the operator log. The gateway maps -32000 to a
  non-retryable 400, so a node fault would have looked like the client's
  mistake.
- **Pre-existing, found here, fixed.** `rpc::limit::RateLimiter` was built
  and tested but never wired in. `rpc.rate_limit_per_second` (50) and
  `rate_limit_burst` (100) were read from the config and printed at startup,
  and nothing enforced them, while `maya2c-node` binds `0.0.0.0:8545` by
  default. `serve_metered` now runs its own accept loop so each request is
  checked against the caller's address, and refuses with HTTP 429.
  `rpc_tests::a_caller_over_the_rate_limit_is_refused_with_429` checks that
  ten immediate calls at 2/s burst 3 admit 3 or 4, and that 0 disables it.
  `sdk-e2e` and `mesh-check` both still pass with it on.
  **Operator note:** `maya2c-gateway` and `maya2c-mesh` on the same host
  reach the node from 127.0.0.1, so every client behind them shares one
  bucket. Size the limit for that, or set 0 where a gateway fronts the node.

The driver is Rust (`xtask/src/mesh_check.rs` over `xtask/src/devnet.rs`),
not a script. `mesh-cli` itself is a downloaded Go binary
(`D:/Tools/mesh-cli`), passed with `--mesh-cli`.

