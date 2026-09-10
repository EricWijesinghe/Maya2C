#!/usr/bin/env bash
# Documentation coverage for every library crate in the workspace.
#
#   scripts/doc_coverage.sh           report per-crate coverage
#   scripts/doc_coverage.sh --check   also fail if any crate is under the floor
#   scripts/doc_coverage.sh --open    build the docs and open them afterwards
#
# On a machine short of memory, rustdoc's parallel search-index writes can fail
# with "Not enough memory resources" (os error 8). CARGO_BUILD_JOBS=1 fixes it.
#
# Two things are measured, because either alone misleads:
#
#   1. Coverage: the share of public items with a doc comment, from rustdoc's
#      own `--show-coverage`. That flag is nightly-only (`-Z unstable-options`);
#      on a stable toolchain this script says so and stops, rather than printing
#      a number it did not measure.
#   2. Integrity: `cargo doc --workspace --no-deps` with broken intra-doc links
#      denied. 100% coverage of docs that link to nothing is not documentation.
#
# The floor is DOC_COVERAGE_FLOOR (percent, default below). It was set from the
# first measurement, not guessed: on 2026-09-10 the workspace measured 99%
# (4,016 of 4,033 public items) and the lowest crate, maya-cuda-miner, 91%. So
# the floor is 90. Raise it when coverage rises; never lower it to make a failure
# go away.
set -euo pipefail

FLOOR="${DOC_COVERAGE_FLOOR:-90}"
CHECK=0
OPEN=0
for arg in "$@"; do
  case "$arg" in
    --check) CHECK=1 ;;
    --open) OPEN=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")/.."

if ! rustc --version | grep -q nightly; then
  echo "doc_coverage: rustdoc --show-coverage needs a nightly toolchain; $(rustc --version) is not one." >&2
  echo "doc_coverage: run with 'cargo +nightly' available, or skip coverage and run: cargo doc --workspace --no-deps" >&2
  exit 3
fi

# Library crates only: a binary's items are not an API anyone reads docs for.
mapfile -t CRATES < <(
  cargo metadata --no-deps --format-version 1 |
    python -c '
import json, sys
meta = json.load(sys.stdin)
members = set(meta["workspace_members"])
for pkg in meta["packages"]:
    if pkg["id"] in members and any("lib" in t["kind"] for t in pkg["targets"]):
        print(pkg["name"])
' | tr -d '\r' | sort
)

printf '%-26s %8s %8s %8s\n' crate items documented coverage
failed=()
total_items=0
total_docs=0
for crate in "${CRATES[@]}"; do
  # Older nightlies print the coverage JSON; newer ones write it to
  # target/doc/<crate>.json and print only where. Read whichever happened.
  out_file="target/doc/${crate//-/_}.json"
  rm -f "$out_file"
  json=$(cargo rustdoc -q -p "$crate" --lib -- -Z unstable-options --show-coverage --output-format json 2>/dev/null) || {
    printf '%-26s %8s\n' "$crate" "(rustdoc failed)"
    failed+=("$crate")
    continue
  }
  if [ -f "$out_file" ]; then json=$(cat "$out_file"); fi
  read -r items docs < <(printf '%s' "$json" | python -c '
import json, sys
data = json.loads(sys.stdin.read())
print(sum(v["total"] for v in data.values()), sum(v["with_docs"] for v in data.values()))
' | tr -d '\r')
  total_items=$((total_items + items))
  total_docs=$((total_docs + docs))
  if [ "$items" -eq 0 ]; then pct=100; else pct=$((docs * 100 / items)); fi
  printf '%-26s %8d %8d %7d%%\n' "$crate" "$items" "$docs" "$pct"
  if [ "$pct" -lt "$FLOOR" ]; then failed+=("$crate ($pct%)"); fi
done
if [ "$total_items" -gt 0 ]; then
  printf '%-26s %8d %8d %7d%%\n' TOTAL "$total_items" "$total_docs" $((total_docs * 100 / total_items))
fi

echo
echo "doc_coverage: building the workspace docs with broken intra-doc links denied"
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --workspace --no-deps -q

if [ "$OPEN" -eq 1 ]; then
  cargo doc --workspace --no-deps --open -q
fi

if [ "$CHECK" -eq 1 ] && [ "${#failed[@]}" -gt 0 ]; then
  echo "doc_coverage: under the ${FLOOR}% floor or failed to document: ${failed[*]}" >&2
  exit 1
fi
