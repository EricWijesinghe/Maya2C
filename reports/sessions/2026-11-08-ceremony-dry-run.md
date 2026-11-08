# Mainnet genesis ceremony — dry run

2026-11-08. Throwaway keys in `D:\Temp\ceremony-dry-run` (off the repo drive), nothing
published, no real key touched. Fills the gap PR #374 left: "NOT VERIFIED: a real ceremony
output run through genesis-check end to end."

Hardware: Windows 11 Pro, the owner's PC. Binaries: `maya2c-node` built
`--profile ci --no-default-features --features production` (the release-check build);
`genesis-ceremony`, `l1-wallet`, `xtask` dev; `maya2c-gpu-miner` release.

## Result

The decided mainnet parameters produce a genesis that `genesis-check` **passes**, and seven
validators built from it finalize blocks on `maya-mainnet`. Two RPC/output gaps surfaced; both
are for the genesis change to close, neither blocks the ceremony. The 20% operator cap does **not**
refuse the single-PC launch.

## Inputs (decided values, DECISIONS 2026-11-07)

- chain id `maya-mainnet`, `--supply 0`
- 7 validators, sealed ML-DSA-87 keys (`maya2c-node --generate-validator-key`, 2592-byte public
  keys), each a distinct operator payout address, each bonded exactly `--min-self-bond`
- `--min-self-bond 1000000000000000000000000000` (1,000 coins; 24 decimals, ADR-068, confirmed
  in code: `maya_amount::ONE_COIN = 10^24`)
- `--mining-reserve 122753829695359067419575633036805965021648` — the schedule's lifetime
  emission, priced from `deploy/genesis/mainnet/mining.json` at 3,600-block epochs, not copied by
  hand (helper `genesis_check::mining_reserve`, pinned to the node's figure by test)
- 2-of-3 security council, `--council-max-pause-blocks 21600` (ADR-050, 6 h of blocks)
- `--min-delegation 1000000000000000000000000` (1 coin)

## Real output

Ceremony (`genesis-ceremony`, `--min-self-bond` = 1,000 coins):

```
chain id       maya-mainnet
total supply   122753829695366067419575633036805965021648
state root     af83d48c7ece342209e60c106763da11cbd69adcf21facf98fdfc969e620a88e
genesis block  73887e14996a3af2cedb837b14433e7db0d8fc294301c5bba89a196a0dd66370
Security council: 2 of 3, a pause lasts at most 21600 blocks
```

`total supply` check: reserve `122753829695359067419575633036805965021648` + 7 × 10^27
(seven 1,000-coin bonds = 7000000000000000000000000000) = `122753829695366067419575633036805965021648`. Exact.

genesis.json: 7 bonds, 7 distinct operators, every bond `1000000000000000000000000000`;
`protocol_upgrades` and `suite_deprecations` empty (omitted); `anchor_timeout_ms` 1000 (≥ 500);
`shielded_activation_height` u64::MAX; fee floor `min_base_fee` 1000000; `stake_weighted` true.

genesis-check:

```
genesis-check: PASS (D:\Temp\ceremony-dry-run\rehearsal\out/genesis.json)
```

Seven validators from that genesis (the `ceremony-rehearsal` xtask, which re-runs the keys +
ceremony + check + network in one command; genesis id differs per run because keys and timestamp
differ):

```
genesis id    "387c4bc32b55b4e95015aa14566a8800f6fcc7a6e3a360020f01ce28e068e98f"
[ok] 7 validators agree on block cd2c…da99 at height 5, 6.57 s after start
bft status {"committed_round":12,"committee":7,"epoch":0,"follower":false,"round":13,"validator":0}
[ok] get_chain_info chain_id "maya-mainnet"
[ok] accounts 122753829695359067419575633036805965021648
     + staked 7000000000000000000000000000
     = 122753829695366067419575633036805965021648  == reserve + 7 x 1000000000000000000000000000
[ok] all 7 still agree
```

Blocks finalized: the run reached height 1849 before it was stopped (about one block a second;
the 15:40 sample showed height 139 two minutes after start). Committee size 7, no follower, no
fork — one chain throughout.

## Findings

### Not a blocker, but for the genesis change

1. **`get_supply` omits bonded stake.** RPC `get_supply` sums account balances only, so it reports
   the reserve alone (`…359067…`) and leaves out the 7 × 1,000 coins that genesis counts in its
   total supply (`…366067…`). The money is not missing — `get_validators` shows every bond — but a
   public "total supply" number read from `get_supply` would understate it by 7,000 coins. Code:
   `crates/node/src/rpc/market.rs::supply` walks accounts; `GenesisConfig::total_supply` adds
   `staking.bonded()`. The rehearsal's supply check now reads accounts + `get_validators`, and
   asserts the `get_supply` shortfall explicitly so the gap stays visible.

2. **`maya-mainnet` has no address text yet (ADR-081, expected).** `Network::for_chain_id` does not
   map `maya-mainnet`, so the node serves hex only and `address_text` is null — no `maya1…` form.
   ADR-081 §4 says the mainnet id is registered "in the same change that fixes mainnet genesis", and
   the test `mainnet_chain_id_has_a_prefix` is `#[ignore]` until then. The rehearsal's `maya1…`
   check fails on purpose to keep this step on the genesis-day list.

### The 20% operator cap does NOT block the launch (coordinator's question)

Seven validators run by one person, each with its **own operator payout address**, hold 1/7 =
14.3% of stake each — under the 20% cap — and genesis-check passes. The README already states this
("The check counts payout addresses … each gets its own operator address"). The cap would refuse
only if one operator address were reused for ≥ 2 of the 7 seats (2/7 = 28.6% > 20%). So the decided
single-PC disclosed launch (ADR-047) is possible as written; no reconciliation needed.

### genesis-check enforces the staking floor; the ceremony does not

With `--min-self-bond` at 100 coins (the figure in the original task), `genesis-ceremony` writes
the genesis without complaint, and `genesis-check` then refuses it:

```
genesis-check: FAIL
  staking.min_self_bond 100000000000000000000000000 is below 1000000000000000000000000000
```

The decided bond is 1,000 coins (the floor in `xtask/src/genesis_check.rs`), not 100. Always run
genesis-check after the ceremony — it is the gate, the ceremony is not.

## Mining reward — NOT VERIFIED in this ceremony run

Mining opens at the first epoch boundary, block 3,600 (~60 min at one block a second). The full
`ceremony-rehearsal` run was stopped at height 1849 to free the CPU for a scheduled attacknet run,
so the share-paid-from-reserve check did not complete here. That exact property — a submitted share
is paid out of the keyless reserve and nothing is minted (the reserve falls by at least what the
miner holds) — is already proven by `cargo xtask mining-localnet`, on the same reserve mechanism
with four validators and the release miner. The ceremony-rehearsal mining stage (`--no-mining` to
skip it) reuses that check at 7 validators; run it once without the attacknet contending for CPU to
close this line.

## Doc fixes made

- `deploy/genesis/mainnet/README.md`: `--council-max-pause-blocks 21600` (was `<N>`); a line
  pricing it (ADR-050, 6 h); the stale 2026-10-30 dry-run section replaced with this run's figures
  and the two findings above; `ceremony-rehearsal` named as the rehearsal command.
- `docs/runbooks/regenesis-checklist.md`: dropped `--difficulty-bits` from the assemble command
  (DAG-BFT writes difficulty 0 regardless); noted the `--timestamp` ±window genesis-check enforces;
  added the ADR-081 `for_chain_id` registration step; pointed at `ceremony-rehearsal`.

## How to rehearse again

```
cargo xtask ceremony-rehearsal                 # full, includes the ~60 min mining stage
cargo xtask ceremony-rehearsal --no-mining     # genesis + 7-validator finalize only (~15 s + build)
```

Throwaway keys and logs land in `%TEMP%\ceremony-rehearsal`, wiped on the next run. Nothing it
writes is a mainnet key.
