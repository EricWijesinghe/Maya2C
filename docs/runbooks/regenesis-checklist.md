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
| ML-DSA-87 validator keys (ADR-051 option 3) | key type is fixed in genesis | built (feat/validator-mldsa87, spec CRY-4); every key must be regenerated with the new binary |
| 20% per-operator cap, 7+ validators | policy | `cargo xtask genesis-check FILE` |
| `--trusted-block` required (production build) | bootstrap safety | #188 |
| Multisig spends need exactly `m` approvals (#196) | a block that used to be valid (spare approvals) is now refused | `multisig_tests`; live history not checked (step 1.4) |
| Transaction size 4 MiB (`MAX_TX_BYTES`), blob lengths via `read_byte_len`, vertex payload cap = `MAX_TX_BYTES` (#196, #201) | block/vertex/snapshot/undo decoding changes; mixed versions disagree on a large vertex | `wire` tests, `bft_node_tests` |
| SettleDvp gated off, `dvp_activation` (ADR-061) | DvP was a stub the seller could not consent to | `features.toml` stub |
| Channels in the undo journal (#196) | reorg across a channel op | undo tests |
| Audit fixes (#200): identity revocation index, vault selection, preview key, `import_headers` refusals, DEX escrow `checked_sub` | selection and acceptance rules | `reports/sessions/2026-10-26-audit-findings.md` |
| **Undecided, would also need this genesis:** ADR-062 (sparse accounts tree, per-key staking), ADR-063 (header version byte, timestamp bound), DEX dust rule (H-1), governance quorum denominator (H-2), retarget window (audit 7) | each changes consensus bytes | audit report; Eric decides |

## 1. Before anything (no approval needed)

1. `cargo xtask genesis-check <genesis.json>` passes on the candidate file.
2. `cargo xtask release-check` passes.
3. `cargo xtask go-no-go`: every FAIL is either fixed or listed as waived in
   DECISIONS.md (2026-10-25).
4. **Multisig history (not checked yet):** scan the live chain for a multisig
   spend carrying more approvals than its policy's `m`. #196 makes such a block
   invalid; it does not matter on a re-genesised chain, but it matters if anyone
   ever replays or imports the old history.
5. The chain-id is new. A reused id with new rules makes two chains look the
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

Amounts are base units, 24 decimals (ADR-068): one coin is 10^24. **The chain's total supply is
`--supply` + `--mining-reserve` + the sum of the bonds**, not `--supply` alone
(found 2026-10-26: a 21M `--supply` plus a 20M reserve gave 41M). For a 21M-coin
cap, pick `--supply` as the small root allocation and set `--mining-reserve` =
cap - supply - bonds. Check the printed `total supply` line before signing. `validators.txt` lines are `label key
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

## Pre-audit LOW-7 and INFO-8 (2026-10-29)

- **Fee floor:** the protocol minimum `MIN_BASE_FEE_FLOOR` is 1 base unit. At 24 decimals that
  is 10^6 times cheaper per byte than 1 unit was at 18. The mainnet genesis sets
  `bft.fees.min_base_fee` to at least `1000000` (the 18-decimal equivalent of 1 unit); the
  constant stays 1 so test genesis files keep working.
- **Supply:** there is no protocol-wide cap. Total supply is exactly what genesis allocates:
  `--supply` + `--mining-reserve` + bonds, printed as `total supply` before signing. With a tail
  (ADR-068), the reserve must hold the lifetime emission, so the total is far above 21M coins
  by design; 21M is the halving schedule's main issuance, and the rest is released only by the
  tail. Check both printed lines before `APPROVED: sign genesis`.
- Tail (DECISIONS 2026-10-29): `tail_epoch_reward` = 1% of 21M coins a year divided by
  epochs per year at the mainnet epoch length, in base units (10^24 per coin).
- **Remote signer database (HIGH-4 review):** votes are now bound to the chain, so a signer's
  slashing-protection database must be per chain. Start mainnet with a fresh signer database;
  reusing a testnet one makes the signer refuse mainnet votes as conflicts (safe, but no votes).
