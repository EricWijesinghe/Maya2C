---
title: Check the foundation
description: Signed release statements, a tamper-evident incident log and a canary, each checkable with one command.
---

Three statements the foundation makes, in a form you can check yourself with
`maya2c-trust` (source: `apps/trust-log`). Each is signed with ML-DSA-87 and
its digest can be anchored on chain, so the record has a height nobody can
move back. You pin the foundation's public key once; a record that brings
its own key is refused.

## Releases

```sh
maya2c-trust release verify --pubkey foundation.pub --tag v1.0.0 \
  --sums SHA256SUMS --statement v1.0.0.release.json \
  --rpc http://127.0.0.1:8545 --anchor-height H --anchor-txid TXID
```

Every check also needs the anchor location, or an explicit `--skip-anchor`.
Fails with a warning if `SHA256SUMS` was swapped, if the statement is for
another tag, or if the anchor is not on chain. It never installs or deletes
anything.

## Incidents

```sh
maya2c-trust incident verify --pubkey foundation.pub --dir reports/incidents/log
```

Every record names the one before it. An edited, removed or reordered record
fails. Removing the newest records is caught against the newest anchored head, or
with `--min-seq` set to the last record number you saw.
Each record lists `maya2c replay` lines for the heights involved.

## Canary

```sh
maya2c-trust canary check --pubkey foundation.pub --rpc http://127.0.0.1:8545 \
  --canary canary.json --max-age-blocks N
```

A canary names a recent block id, so it cannot be signed before that block
exists. If it is not renewed within `N` blocks, `check` fails: the canary has
lapsed.

## Limits today

- The tool prints the anchor fields to submit; it does not submit the
  transaction itself.
- The block is taken from the RPC endpoint you name. Use a node you run.
- These are tools for anyone to check with. Nobody outside the foundation
  runs them yet.
