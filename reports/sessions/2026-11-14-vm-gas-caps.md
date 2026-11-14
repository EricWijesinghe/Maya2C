# 2026-11-14 — VM gas caps and storage pricing (ADR-142)

Eric 2026-11-14 MCQ: "cap + price now". Branch `refreeze/vm-gas-caps`, from
`a0ee0ad6`.

## What changed

- `crates/vm/src/gas.rs`: prices, charged in fuel before the work — 50,000 per
  call, 450 per module byte cold / 1 warm, 600 per host call, storage read
  5,000 + 1/B, write 5,000 + 240/B, event 600 + 1/B. `Vm::compile`,
  `Vm::execute_compiled`, `Compiled`.
- `crates/governance/src/params.rs`: `VmMaxTxGas` (17), `VmMaxBlockGas` (18),
  250 M default, bounds `[239,979,600, 500,000,000]`.
- Node: caps in `call_contract` / `deploy_contract` and at mempool admission;
  per-block `Overlay::vm_gas`; warm modules in `Overlay::vm_modules`; one `Vm`
  per `StateDB`; `StateDB::db` is `Arc<DB>`; `ChainHost` reads committed
  storage per key; failed revealed sealed calls keep their gas spent.
- `spec/10-vm-gas.md` (GAS-1..6), `spec/tests/vm_gas.json` (26 cases).

## Commands and output (this machine, dev profile)

`cargo test -p maya-vm --test gas_price_calibration_tests -- --nocapture --test-threads 1`
(before the prices existed):

```
CAL cold call, 266052 B module               114.948 ms     432.1 ns/byte
CAL warm call, empty contract               49.500 us
CAL guest: empty loop                       0.113 ms  fuel       160002        5.6 ns/call     0.706 ns/fuel
CAL guest: memory.fill 64 KiB              13.774 ms  fuel   1310960002      688.7 ns/call     0.011 ns/fuel
CAL host: block_height                      1.863 ms  fuel       180002       93.2 ns/call    10.350 ns/fuel
CAL host: get_balance_u256                 11.250 ms  fuel       220002      562.5 ns/call    51.137 ns/fuel
CAL host: storage_write 128 B + 4 KiB      52.484 ms  fuel       324099     2624.2 ns/call   161.938 ns/fuel
CAL host: emit_event 4 KiB + 4 KiB (x64)      0.168 ms  fuel          834     2621.9 ns/call   201.199 ns/fuel
```

Second compile probe: many functions 292–298 ns/B, one function 222–234 ns/B.

Red: `cargo test -p maya-vm --test gas_pricing_tests` before the
implementation: `error[E0432]: unresolved import maya_vm::gas`, and no
`compile` / `execute_compiled` on `Vm`. The node red run raced the
implementation edits and failed in the lib (`no variant VmMaxTxGas`), so it is
not a clean red for the node tests.

`cargo test -p custom-l1-node --test vm_gas_timing_tests -- --nocapture --test-threads 1`:

```
TIMING storage reads (RocksDB hits)            78.4 ms   10 txs  gas   250000000   0.313 ns/gas
TIMING guest compute (i64.div_u)               36.5 ms    1 txs  gas   250000000   0.146 ns/gas
TIMING host calls (get_balance_u256)          229.4 ms   10 txs  gas   250000000   0.918 ns/gas
TIMING   (230 writes of 4 KiB: 920 KiB of new contract state)
TIMING storage writes (4 KiB)                   9.7 ms   10 txs  gas   250000000   0.039 ns/gas
TIMING deploys (compile)                      249.6 ms   10 txs  gas   243923000   1.023 ns/gas
TIMING   (10 modules of 54094 B)
TIMING cold calls (compile)                   230.6 ms   10 txs  gas   243923000   0.945 ns/gas
test result: ok. 5 passed; 0 failed
```

Green runs:

- `cargo test -p maya-vm --no-fail-fast`: every target ok after three tests
  that measured gas differences across modules of different lengths were
  corrected for the new per-byte call price (`property_tests`,
  `tensor_gas_tests`, `zkml_host_tests`); `tier_differential_tests` 2 ignored
  on Windows (existing FINDING), so Pulley parity is CI's (Linux).
- `cargo test -p custom-l1-node --test vm_gas_tests`: 7 passed.
- `cargo test -p custom-l1-node --lib vm_exec`: 2 passed (randomized lazy
  storage equivalence over 40 storages; cap order).
- `cargo test -p custom-l1-node --no-fail-fast --lib --test pruned_node_tests --test neural_fee_tests --test exploit_replays --test block_bytes_tests`:
  lib 544 passed, 1 ignored; 10, 11, 10, 19 passed.
- `cargo test -p custom-l1-node --test contract_tests --test nft_game_on_node_tests --test oracle_tests`: 12, 1, 30 passed.
- `cargo test -p maya-governance`: 30, 8, 2 passed.
- `cargo test -p maya2c-cli --no-fail-fast`: all ok, no failures.
- `cargo xtask spec-coverage --strict`: `68 rules, 190 vectors; 65 consensus rules covered, 0 gaps`.
- `cargo xtask claims-check`: `97 public files scanned, 0 unauthorised claims`.

## Not run

- `cargo xtask coverage --verify-targets`, workspace clippy ratchet: left to CI
  (AGENT-RULES).
- Release-profile timing: not measured; dev-profile figures are pessimistic.
- Full `custom-l1-node` integration suite: only the targets above.
