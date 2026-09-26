#!/usr/bin/env bash
#
# A ratchet for clippy::pedantic debt.
#
# `[workspace.lints] clippy::pedantic = "warn"` was switched on during the
# foundation phase and produced 1,700 diagnostics, dominated by the cast lints
# and `doc_markdown`. Denying them would have meant either a 1,700-diagnostic
# cleanup inside a change about workspace layout, or a gate no branch could
# pass (docs/adr/ADR-003-build-profiles.md).
#
# Warned-but-untracked debt is debt that grows. This is the middle position:
# the number may go down freely and may not go up. When a change legitimately
# adds warnings — a new crate, or a clippy release that adds a lint — run
# `--update` and say so in the commit message, so the increase is a decision
# somebody made rather than one nobody noticed.
#
# Not in ci.yml: each distinct set of clippy flags is a distinct fingerprint,
# so this is a third full clippy pass and the debt does not need to block a
# pull request. `.github/workflows/nightly.yml` runs it.
#
#   scripts/lint_debt.sh              report the count
#   scripts/lint_debt.sh --check      fail if it is above the baseline
#   scripts/lint_debt.sh --update     write the current count as the baseline

set -euo pipefail

cd "$(dirname "$0")/.."
BASELINE_FILE="lint-debt.txt"
MODE="${1:---report}"

if [[ ! -f "$BASELINE_FILE" ]]; then
    echo "lint_debt: $BASELINE_FILE is missing; run --update to create it" >&2
    exit 1
fi

baseline=$(grep -oE '^[0-9]+' "$BASELINE_FILE" | head -1)

echo "lint_debt: running clippy across the workspace (this is a full pass)"
# `|| true` on the pipeline: clippy exits non-zero only on errors, and warnings
# are the point here. A real error still shows up as a zero count, which the
# sanity check below catches.
output=$(cargo clippy --workspace --all-targets --message-format short 2>&1 || true)
count=$(printf '%s\n' "$output" | grep -cE ':[0-9]+:[0-9]+: warning:' || true)

# Both shapes: a located diagnostic (`file:line:col: error`) and cargo's own
# summary (`error: could not compile`). A deny-by-default clippy lint can show
# only the second, and missing it once made a crate that failed to lint vanish
# from the count: 346 "fewer" warnings that were really a compile error.
if printf '%s\n' "$output" | grep -qE ':[0-9]+:[0-9]+: error|^error: could not compile'; then
    echo "lint_debt: clippy reported errors, not just warnings:" >&2
    printf '%s\n' "$output" | grep -E ':[0-9]+:[0-9]+: error|^error' | head -10 >&2
    exit 1
fi

echo "lint_debt: $count diagnostics (baseline $baseline)"

case "$MODE" in
    --update)
        cat > "$BASELINE_FILE" <<EOF
$count
# clippy::pedantic diagnostics across \`cargo clippy --workspace --all-targets\`.
#
# Written by scripts/lint_debt.sh --update. The number may go down freely and
# may not go up: scripts/lint_debt.sh --check fails if it does, and
# .github/workflows/nightly.yml runs that check.
#
# If a change legitimately raises it — a new crate, or a clippy release that
# adds a lint — re-run --update and say so in the commit message. An increase
# should be a decision somebody made, not one nobody noticed.
EOF
        echo "lint_debt: baseline updated to $count"
        ;;
    --check)
        if (( count > baseline )); then
            echo "lint_debt: FAIL — $count diagnostics, up from $baseline (+$((count - baseline)))" >&2
            echo "lint_debt: fix them, or run scripts/lint_debt.sh --update and justify the rise" >&2
            exit 1
        fi
        if (( count < baseline )); then
            echo "lint_debt: $((baseline - count)) fewer than the baseline — run --update to lock it in"
        fi
        echo "lint_debt: ok"
        ;;
    --report) ;;
    *)
        echo "lint_debt: unknown option '$MODE'" >&2
        exit 1
        ;;
esac
