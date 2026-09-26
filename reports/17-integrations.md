# 17 — The integration layer

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 17

> DONE WHEN: mesh-cli checks pass; the deposit service passes 100,000
> deposits with restarts; the offline signing loop and WalletConnect flow pass
> e2e; the 5-minute developer path is measured on three OSes; every guide's
> commands run in CI; reports/17-integrations.md is complete.

| Condition | Result |
|---|---|
| mesh-cli `check:data` / `check:construction` | **not done.** No Mesh (Rosetta) API is implemented |
| Deposit service, 100,000 deposits with restarts | **passes**: 100,090 deposits, 40 SIGKILLs |
| Offline signing loop, e2e | **passes** |
| WalletConnect flow, e2e | **not done.** No WalletConnect or provider API |
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
    multisig is the supported path (and it is dark today);
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
