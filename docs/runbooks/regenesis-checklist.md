# Re-genesis checklist (pre-mainnet testnet, then mainnet)

One checklist for both: the testnet re-genesis rehearses the exact steps of
the mainnet genesis. Steps marked **APPROVED** need Eric's written
`APPROVED: <step>` first (CLAUDE.md order 6). Nothing here is run by Claude
against a live server on its own.

## 0. What this genesis carries that the old testnet did not

| Item | Why it needs a fresh chain | Evidence it works |
|---|---|---|
| `bft.mining` (ADR-044), funded keyless reserve | mining has never run on the live testnet | `cargo xtask mining-localnet` |
| Security council 2-of-3 (ADR-050) | the old genesis has none | `genesis-ceremony council ...` |
| wasmtime 48.0.7 | consensus-VM bump, a hard fork | #190, vm tests |
| Staking record v3, stake added in an epoch earns from the next | state format change | PR for `fix/reward-sniping` |
| ML-DSA-87 validator keys (ADR-051 option 3) | key type is fixed in genesis | not built yet |
| 20% per-operator cap, 7+ validators | policy | `cargo xtask genesis-check FILE` |
| `--trusted-block` required (production build) | bootstrap safety | #188 |

## 1. Before anything (no approval needed)

1. `cargo xtask genesis-check <genesis.json>` passes on the candidate file.
2. `cargo xtask release-check` passes.
3. `cargo xtask go-no-go`: every FAIL is either fixed or listed as waived in
   DECISIONS.md (2026-10-25).
4. The chain-id is new. A reused id with new rules makes two chains look the
   same to a wallet.

## 2. Keys (Eric present; Claude never holds a key)

1. One validator key per validator: `maya2c-node --generate-validator-key PATH
   --validator-passphrase-file F` on the machine that will run it. Sealed from
   creation; the passphrase file lives off the repo drive.
2. Three council keys on three devices: `genesis-ceremony council key --label L
   --passphrase-file F`. Copy only `*.council.public` into the council folder;
   the tool refuses a folder holding a `.sealed` file.
3. Back up each sealed key and its passphrase in two places before genesis.

## 3. Assemble (Eric runs; output is public except `*.secret`)

```
genesis-ceremony --chain-id <ID> --supply <UNITS> \
  --validators validators.txt --mining mining.json --mining-reserve <UNITS> \
  --council council-public --council-threshold 2 --council-max-pause-blocks <N> \
  --timestamp <UNIX> --difficulty-bits <N> --out-dir out
```

Amounts are base units, 18 decimals. `validators.txt` lines are `label key
operator bond`; the bonds are the disclosed validator-bond allocation, not a
premine. Then `cargo xtask genesis-check out/genesis.json`, commit the file,
and have Eric sign it (`genesis/<chain>.json.sig`). **APPROVED: sign genesis.**

## 4. Start (testnet) **APPROVED: re-genesis**

1. Stop the old validators by PID and install path, never by image name.
2. Archive the old data dir; do not delete it until the new chain is past 2
   epochs.
3. Start the Oracle validators first, then the PC's (`start-pc-validators.ps1`;
   the wipe guard refuses a wipe while the epoch is unknown).
4. Pass `--trusted-block HEIGHT:HEX` for every bootstrap, taken from a node you
   run. A production build refuses to bootstrap without it.
5. Watch `get_bft_status` on each node: epoch advances, committee size is the
   genesis size, nobody is `follower`.

## 5. Host layout and what it survives (be exact in public)

BFT needs more than 2/3 of stake online. A host's failure halts the chain when
that host holds one third or more of the stake. With **three** hosts one of them
always holds at least a third, so any single host outage halts the chain until
it returns. It resumes without a fork. Surviving any single host needs **four**
hosts, each under one third of stake. State which one this launch is.

## 6. Rollback

Until mainnet carries real value, rollback is "start the archived data dir on
the old binary". After that there is no rollback; a bad genesis is a new
chain-id.
