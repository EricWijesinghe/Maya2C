#!/usr/bin/env bash
# Offline statistical batteries for hal/entropy — ADR-010.
#
# Runs NIST SP 800-22 (STS 2.1.2) and Dieharder over `entropy-dump` output and
# saves every raw report under reports/entropy/. It never summarises a run as
# "passed": the reports are the result, and reports/02-crypto.md reads them.
#
# Run inside WSL (or any Linux) from the repository root:
#
#   cargo build --release -p maya-entropy --bin entropy-dump    # on Windows
#   wsl -d Ubuntu-24.04 -u root -- bash scripts/entropy_battery.sh
#
# Requirements: `dieharder` (apt), and NIST STS 2.1.2 built at $STS_DIR
# (https://csrc.nist.gov/projects/random-bit-generation/documentation-and-software).
#
# What is tested, and why only this:
#   - `pool` (the DRBG output keys are made from): STS and the full Dieharder
#     suite. This is the output that must be indistinguishable from random.
#   - raw `os`, `rdseed`, `sim-thermal`, `sim-homodyne-qrng`: STS only, as
#     information. Raw sources are *not* expected to be full-entropy (SP
#     800-90B assesses them by min-entropy, not by these batteries); a SIM
#     result describes a model driven by OS randomness, not any device.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/reports/entropy"
DUMP="${ENTROPY_DUMP:-$ROOT/target/release/entropy-dump.exe}"
STS_DIR="${STS_DIR:-/opt/sts/sts-2.1.2/sts-2.1.2}"
STREAMS="${STS_STREAMS:-100}"
STREAM_BITS=1000000
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

[ -x "$DUMP" ] || { echo "entropy_battery: $DUMP not found; build entropy-dump first" >&2; exit 1; }
[ -x "$STS_DIR/assess" ] || { echo "entropy_battery: NIST STS not built at $STS_DIR" >&2; exit 1; }
command -v dieharder >/dev/null || { echo "entropy_battery: dieharder not installed" >&2; exit 1; }
mkdir -p "$OUT"

sts() { # source
  local src="$1" bytes=$((STREAMS * STREAM_BITS / 8))
  local file="$WORK/$src.bin"
  "$DUMP" "$src" "$bytes" "$(wslpath -w "$file" 2>/dev/null || echo "$file")"
  rm -rf "$STS_DIR/experiments/AlgorithmTesting/"*/*.txt 2>/dev/null || true
  # assess prompts: generator 0 (file), path, 1 (all tests), 0 (default
  # parameters), stream count, 1 (binary).
  ( cd "$STS_DIR" && printf '0\n%s\n1\n0\n%s\n1\n' "$file" "$STREAMS" | ./assess "$STREAM_BITS" >/dev/null )
  {
    echo "# NIST SP 800-22 (STS 2.1.2) — source: $src — $STREAMS streams × $STREAM_BITS bits"
    echo "# $(date -u +%Y-%m-%dT%H:%MZ)"
    cat "$STS_DIR/experiments/AlgorithmTesting/finalAnalysisReport.txt"
  } > "$OUT/sts-$src.txt"
  echo "entropy_battery: STS $src -> $OUT/sts-$src.txt"
}

dieharder_pool() {
  {
    echo "# Dieharder $(dieharder -h 2>/dev/null | sed -n 2p | tr -s ' ') — source: pool (DRBG output), piped via stdin"
    echo "# $(date -u +%Y-%m-%dT%H:%MZ)"
    "$DUMP" pool 1000000000000 - 2>/dev/null | dieharder -a -g 200 || true
  } > "$OUT/dieharder-pool.txt"
  echo "entropy_battery: Dieharder pool -> $OUT/dieharder-pool.txt"
}

for src in pool os rdseed sim-thermal sim-homodyne-qrng; do
  sts "$src" || echo "entropy_battery: STS $src could not run" >&2
done
# SKIP_DIEHARDER=1 for a quick STS-only run; the full suite takes about an hour.
[ "${SKIP_DIEHARDER:-0}" = 1 ] || dieharder_pool
