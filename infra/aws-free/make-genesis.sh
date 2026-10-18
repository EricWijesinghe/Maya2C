#!/bin/bash
# Re-genesis maya-testnet-1 for the AWS validators: the previous genesis with
# only the validator keys and the timestamp replaced. Allocations and bond
# operators stay, so every wallet keeps its address and balance. Bonds pair
# with validators by position (crates/node/src/genesis.rs), so v1 takes the
# seed's bond, v2..v4 the old v2..v4 bonds.
#
#   make-genesis.sh <previous genesis.json> <dir with v1.pub..v4.pub> <out>
set -euo pipefail

prev=${1:?previous genesis}; pubs=${2:?pub dir}; out=${3:?output}

keys=()
for n in 1 2 3 4; do
    key=$(tr -d '[:space:]' < "$pubs/v$n.pub")
    [[ $key =~ ^[0-9a-f]+$ ]] || { echo "v$n.pub is not hex" >&2; exit 1; }
    keys+=("$key")
done

count=$(jq '.bft.validators | length' "$prev")
[ "$count" = 4 ] || { echo "previous genesis has $count validators, expected 4" >&2; exit 1; }

jq --argjson ts "$(date -u +%s)" --args \
    '.timestamp = $ts | .bft.validators = $ARGS.positional' \
    "${keys[@]}" < "$prev" > "$out"
echo "wrote $out (timestamp $(jq .timestamp "$out"))"
