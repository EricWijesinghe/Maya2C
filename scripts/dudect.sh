#!/usr/bin/env bash
# Runs the dudect timing-leak benches and writes reports/dudect.txt.
#
# Why not `cargo bench --bench dudect`: cargo appends `--bench` to every bench
# binary's argv, and dudect-bencher 0.7's argument parser rejects it
# ("Found argument '--bench' which wasn't expected"). So the binary is built
# with `--no-run` and executed directly, with whatever arguments are given here
# (`--filter <name>`, `--continuous <name>`).
#
# Run it on an idle machine. A parallel build steals cycles unevenly between
# the two input classes and shows up as a leak that is not there.
set -euo pipefail
cd "$(dirname "$0")/.."

exe=$(cargo bench -p maya-crypto-pq --bench dudect --no-run --message-format=json 2>/dev/null \
  | python -c 'import json,sys
for line in sys.stdin:
    try: m=json.loads(line)
    except ValueError: continue
    if m.get("reason")=="compiler-artifact" and m.get("target",{}).get("name")=="dudect" and m.get("executable"):
        print(m["executable"])' | tail -1)
[ -n "$exe" ] || { echo "dudect: could not locate the bench binary" >&2; exit 1; }

if [ "$#" -gt 0 ]; then
  exec "$exe" "$@"
fi

out=reports/dudect.txt
{
  echo "# dudect run — $(date -u +%Y-%m-%dT%H:%MZ), $(rustc --version), release profile"
  echo "# source: crates/crypto-pq/benches/dudect.rs (inputs prepared before measurement; both classes allocated alike) — interpretation in reports/02-crypto.md"
  echo
  "$exe"
} > "$out"
echo "dudect: wrote $out"
