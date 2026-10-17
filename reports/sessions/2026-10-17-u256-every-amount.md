# 2026-10-13…17 — Every amount is 256-bit (ADR-046 amendment, ADR-048)

Eric: "when I return maximum celling for everything should be done and finish".
Branch `feat/u256-supply` (worktree `.claude/worktrees/agent-a4bc2d29fb7122f2b`),
which absorbs PR #122 (`feat/u256-amount-crate`) and supersedes it.

## What changed

- The five proved crates (`ledger-math`, `staking`, `governance`, `fee-market`,
  `dex`) and `htlc-lattice`/`rwa` compute on `maya-amount::Amount`
  (dependency-free, `no_std`). Kani harnesses rewritten with inverse oracles;
  their bounds are stated in each `proofs.rs` and in ADR-048.
- Node: every value path (transfers, fees, DEX, channels, HTLC, vault, RWA,
  governance, staking, mining, genesis, RPC) is 256-bit. Records that changed
  width bump their version (staking 2, vault 2, mining 3, HTLC lock 3).
- Spec ENC-3/STF-2, `spec-ref`, vectors and the TypeScript verifier follow:
  outputs are 32-byte amounts on the wire.
- Apps: L2 flash channels/routing, l1-wallet CLI (decimal `--amount/--bond/--fee`),
  wallet-gui core, HTLC watcher, explorer (TEXT column), GPU miner, Hub, the
  Ledger device app (32-byte parse, 78-digit display).
- What stays 64-bit, and why: ADR-048's table (nonces/counts, governed
  parameters, per-byte base fee, BFT weight with uniform scaling, shielded
  public values, RWA cap-table units, ISO 20022, faucet/pool internals, the
  GUI wallet's Leptos screens).

## Evidence (real output, this worktree)

```
cargo check --workspace --all-targets            Finished, no warnings
cargo nextest run --workspace --no-fail-fast     Summary [403.343s] 3236 tests run: 3236 passed (13 slow), 11 skipped
cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::unwrap_used -D warnings   Finished
cargo clippy --workspace --lib --bins -- -D clippy::unwrap_used                                    Finished
cargo run -p maya-spec-ref --bin spec-vectors -- --check                                           exit 0
node spec/verifier-ts/verify.ts      {"state_transitions":20,"encoding":9,"fees":17,"headers":3,"keys":4} cases, 0 mismatches
apps/ledger-maya2c  cargo test       all suites ok (parity 4 passed)
apps/hub/src-tauri  cargo test       9 passed
```

## NOT VERIFIED

- Kani proofs: not run (WSL only on this machine; CI nightly runs them).
- `scripts/lint_debt.sh --check`, `scripts/doc_coverage.sh --check`,
  `cargo xtask coverage`: started, killed by the OS for low memory; left to CI.
- `apps/wallet-gui/src-tauri` and `apps/wallet-gui/ui` (non-members): edited,
  not compiled.
- Testnet: the wire and state formats changed; the live testnet needs a
  re-genesis before it can run this code. Not done — it touches the live seed.

## Red team tried

Overflow at every new edge (2^256 − 1 sums in outputs, genesis, tallies,
channel totals, fee+amount in the wallet); old-format records (refused by
version byte); decoder bounds (`OUTPUT_SIZE`/`CLOSURE_SIZE` were still 8-byte
and are fixed — they gate allocation before reading); the pool-value anomaly
check at 2^255 reserves (1088-bit cross-multiplication, rounding unchanged).
