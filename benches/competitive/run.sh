#!/usr/bin/env bash
# Competitive benchmark harness (Master Prompt 21 §4).
#
# Runs the same workload against other chains' local devnets where their
# binaries are installed and their licences allow, and records versions and
# dates. It never compares Maya2C lab numbers with another chain's production
# numbers: every row says where it ran.
#
# Today it runs no other chain end to end: no devnet binary (bitcoind, geth,
# solana-test-validator, sui) is installed on the machine that wrote it, and
# each prints SKIPPED. What it does run is the primitive-level comparison
# that needs no other chain: signature verification per core for Ed25519
# (the scheme Solana, Sui and Aptos sign with) beside Maya2C's hybrid.
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT="benches/competitive/results-$(date -u +%Y-%m-%d).txt"
{
  echo "date:     $(date -u +%Y-%m-%dT%H:%MZ)"
  echo "commit:   $(git rev-parse --short HEAD)"
  echo "cpu:      $(lscpu 2>/dev/null | awk -F: '/Model name/ {gsub(/^ +/,"",$2); print $2}') x $(nproc)"
  echo "rustc:    $(rustc --version)"
  echo
  for chain in bitcoind geth solana-test-validator sui; do
    if command -v "$chain" >/dev/null 2>&1; then
      echo "$chain: $("$chain" --version 2>&1 | head -1) — end-to-end workload not yet scripted"
    else
      echo "SKIPPED  $chain devnet: binary not installed"
    fi
  done
  echo
  echo "== primitive level (same machine, same run; not a chain comparison) =="
  cargo bench -q -p maya-crypto-pq --bench certificate 2>/dev/null | sed -n '/verifications per second/,/^$/p'
} | tee "$OUT"
echo "wrote $OUT"
