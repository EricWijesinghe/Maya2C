# 2026-10-31 l1-wallet: confirm, cap fees, pin collector and genesis

Branch `fix/l1-wallet-confirm`, off master. Threat model P4 (CLI signs without
confirmation, node-chosen collector, fee up to u64::MAX) and P3 (genesis
trusted from the node) for the CLI.

## Changes

- `bins/l1-wallet/src/guard.rs` (new). Pure rules, each unit-tested:
  - `check_genesis`: an explicit `--genesis` must equal the node's genesis.
    A pinned id (mainnet table, testnet table) is trusted. A claimed mainnet
    name is refused unless its genesis is the pinned mainnet one, even with
    `--genesis`. Anything else needs `--genesis`.
  - `MAINNET_GENESIS` empty (until the ceremony). `TESTNET_GENESIS` empty
    (see gaps).
  - `check_collector`: the node's collector must equal
    `custom_l1_node::state::fees::FEE_COLLECTOR`.
  - `quote_fee`: `--fee` or the node quote `2 x base_fee x size`, checked
    against the cap. Cap = `--max-fee` or `DEFAULT_MAX_FEE` = 10^18 base units
    (one MAYA, ADR-046 18 decimals). Fixed, so it does not move with the node's
    quote or the amount.
  - `fee_warning`: transfers only, fee above a tenth of the amount. Warn, do not
    refuse.
  - `require_confirmation`: `--yes` skips the prompt. Otherwise without a
    terminal, refuse and read nothing. On a terminal, a typed `yes` is required.
- `bins/l1-wallet/src/main.rs`. `submit` order: genesis, collector and fee
  (`with_fee`), then print the description from the exact transaction to be
  signed (chain, genesis, outputs with full addresses, fee), then confirm, then
  sign. `send` now flattens the shared `TxOptions`, which gained `--max-fee`,
  `--genesis`, `--yes`. `value_moved` adds stake amounts for the warning.
- `bins/l1-wallet/src/client.rs`. `get_network()` returns the full `ChainInfo`
  (name plus genesis). `get_chain_info()` is unchanged in behaviour.
- `bins/l1-wallet/src/lib.rs`. `pub mod guard`.
- `bins/l1-wallet/tests/send_guard.rs` (new). Runs the real `l1-wallet` binary
  against a fake JSON-RPC node that records each method called. Stdin is closed,
  so the child is never on a terminal.
- `docs/site/authored/wallet.mdx`. Command line and "Before a key signs" section.

## Deviations from the brief

- Default cap is a fixed 1 MAYA, not a multiple of the node's quote. A
  quote-relative cap rises with a hostile quote. A tenth-of-amount cap was
  replaced at the coordinator's direction: it made dust transfers and
  zero-amount kinds unusable.
- No testnet genesis pinned. The only id in the repo, `c80cc217...`
  (`reports/incidents/2026-10-13-v4-follower-node.log.txt`,
  `docs/releases/node-v0.1.0-testnet.1.md`), predates the 2026-10-17 re-genesis.
  Pinning it would be wrong. Every testnet needs `--genesis <id>`.
- No wallet config file exists in `l1-wallet`, so the default cap is not
  configurable beyond `--max-fee`.

## Test output

`CARGO_BUILD_JOBS=2 cargo test -p l1-wallet` (after `rustfmt --edition 2024` on
the changed files), exit 0:

```
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.60s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
EXIT=0
```

The 43 unit tests include the guard refusals: unpinned genesis without the
flag, flag mismatch, mainnet name with unpinned genesis even with the flag,
wrong collector, hostile quote above the default cap, base fee near 2^256
(overflow refused), explicit `--fee` above cap, zero-amount kind gets the
default cap, dust transfer pays and warns, non-terminal refused with nothing
read, typed `yes` only.

The 7 integration tests run the binary against the fake node: unpinned genesis
refused and not broadcast, flag mismatch refused, wrong collector refused, quote
above cap refused, no-TTY without `--yes` refused, dust transfer signs with a
warning, `--yes` prints the description and signs (`--no-broadcast`). Each
refusal asserts `send_raw_transaction` was never called.

Not run: `cargo clippy` and the repo lint ratchet (`scripts/lint_debt.sh`) on
`l1-wallet`. Not run: `/verify` before commit.

## Gaps

- No testnet genesis pin. Testnet use needs `--genesis`.
- Mainnet pin table empty until the ceremony.
- No wallet config file; `--max-fee` is the only override.
- 1 MAYA default cap is a judgment, not measured from testnet fee data.
- Staking and council payloads go through the same cap, not a per-kind cap.
- `with_fee` swallows a `get_fee_info` error as "no fees" (existing behaviour,
  unchanged).
- Lint and clippy not run.
