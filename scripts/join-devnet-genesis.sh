#!/usr/bin/env bash
# A throwaway one-validator genesis for the join-script CI (join-scripts.yml).
# Same shape as `xtask devnet` (one validator, one bond, one funded wallet),
# so the node starts from it with no network at all.
#
#   scripts/join-devnet-genesis.sh <bin-dir> <out-dir>
#
# Prints the genesis file's path and sha256 on one line. Devnet only: the keys
# and the wallet password are throwaway and never leave the runner.
set -euo pipefail

BIN=$1
OUT=$2
PASSWORD="devnet-only-password"
BALANCE=10000000
BOND=100000

mkdir -p "$OUT"
EXE=""; [ -f "$BIN/maya2c-node.exe" ] && EXE=.exe
PUB=$("$BIN/maya2c-node$EXE" --generate-validator-key "$OUT/validator.key" | tr -d '[:space:]')
ADDR=$(L1_WALLET_PASSWORD="$PASSWORD" "$BIN/l1-wallet$EXE" --keystore "$OUT/wallet.key" generate \
  | grep '^address:' | cut -d: -f2 | tr -d '[:space:]')
[ -n "$PUB" ] && [ -n "$ADDR" ] || { echo "devnet genesis: missing key or address" >&2; exit 1; }

cat > "$OUT/genesis.json" <<JSON
{"chain_id":"maya-join-ci","timestamp":$(date +%s),"difficulty_bits":0,"pow_limit_bits":0,
 "allocations":[{"address":"$ADDR","balance":$BALANCE}],
 "bft":{"validators":["$PUB"],"anchor_timeout_ms":1000,"batch_size":500,
   "fees":{"initial_base_fee":1,"min_base_fee":1,"target_block_bytes":2621440,"change_denominator":8},
   "staking":{"epoch_blocks":20,"bonds":[{"operator":"$ADDR","bond":$BOND}]}}}
JSON

# Hash from stdin: sha256sum prefixes a backslash when the file NAME has one,
# which a Windows path always does, and that would corrupt the value.
if command -v sha256sum >/dev/null; then SUM=$(sha256sum < "$OUT/genesis.json" | cut -d' ' -f1)
else SUM=$(shasum -a 256 < "$OUT/genesis.json" | cut -d' ' -f1); fi
echo "$OUT/genesis.json $SUM"
