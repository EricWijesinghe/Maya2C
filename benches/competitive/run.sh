#!/usr/bin/env bash
# Competitive benchmark harness (Master Prompt 21 §4).
#
# Runs the same workload against other chains' local devnets where their
# binaries are installed and their licences allow, and records versions and
# dates. It never compares Maya2C lab numbers with another chain's production
# numbers: every row says where it ran.
#
# End to end, where the other chain's devnet is installed: Ethereum via
# Foundry's anvil (eth_anvil_workload.py), the same shape as Maya2C's
# examples/bft_tps.rs — 10,000 transfers from 2,000 accounts. Chains whose
# devnet is absent print SKIPPED. It also runs the primitive-level comparison
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
  if command -v anvil >/dev/null 2>&1 && python3 -c "import eth_account" 2>/dev/null; then
    echo "== end to end: Ethereum (anvil) =="
    python3 benches/competitive/eth_anvil_workload.py 10000 2000
  else
    echo "SKIPPED  anvil: binary or eth-account not installed"
  fi
  if [[ -x target/release/examples/bft_tps ]]; then
    echo "== end to end: Maya2C (examples/bft_tps.rs, 4 validators in one process) =="
    TPS_DIR="${TMPDIR:-/tmp}" target/release/examples/bft_tps 2000 5 | tail -1
  else
    echo "SKIPPED  Maya2C bft_tps: build with cargo build --release -p custom-l1-node --example bft_tps"
  fi
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
