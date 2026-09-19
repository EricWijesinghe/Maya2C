#!/usr/bin/env bash
#
# Gate on undocumented `unsafe` in first-party code.
#
# Usage:
#   ./scripts/check-unsafe.sh
#
# Exits non-zero if any `unsafe` block or `unsafe impl` in this repository's own
# Rust lacks a `// SAFETY:` comment in the five lines above it.
#
# ## Why this exists beside cargo-geiger
#
# `cargo geiger` counts unsafe expressions across the whole dependency graph.
# That is a survey, and it is useful as one, but it cannot be a gate: we do not
# control what RocksDB's bindings or the arkworks stack do, so any threshold
# over that number would be chosen to pass rather than to mean something. It is
# also lightly maintained enough that a build red for geiger reasons is more
# often a tooling problem than a safety problem.
#
# This script gates the half we actually control, deterministically, with no
# dependency beyond grep and awk.
#
# ## Why a comment and not a count
#
# A baseline count rots: it drifts every time an unrelated line moves, and it
# says nothing about whether the unsafe that exists is justified. Requiring a
# `// SAFETY:` note is the rule CLAUDE.md already states ("Never use `unsafe`
# without comments"), and it is the rule that stays true as the code changes.
#
# ## What is checked, and what is not
#
# Checked: `unsafe { ... }` blocks and `unsafe impl`. Both assert a proof
# obligation at the point they appear, so that is where the justification goes.
#
# Not checked: `unsafe fn`, `unsafe extern`, and `#[unsafe(...)]` attributes.
# Those *declare* an obligation rather than discharge one — the caller is who
# has to justify anything — so demanding a SAFETY note on the declaration would
# train people to write one that says nothing.
#
# Not scanned: `contracts/`. Those build for wasm32 against a host ABI, out of
# this workspace and on their own toolchain, and their unsafe is host-import
# plumbing with a different review story. They are counted and reported below so
# the omission is visible, but they are not gated here.

set -euo pipefail

cd "$(dirname "$0")/.."

readonly WINDOW=5

# First-party Rust, minus build output and the wasm contracts.
# NUL-delimited throughout, so a path with a space cannot split into two.
find_sources() {
    find . -type f -name '*.rs' \
        -not -path './target/*' \
        -not -path '*/target/*' \
        -not -path './contracts/*' \
        -print0 \
        | sort -z
}

# Emits `path:line: text` for every unsafe block or impl with no SAFETY comment
# in the preceding $WINDOW lines.
find_undocumented() {
    find_sources | xargs -0 -r awk -v window="$WINDOW" '
        FNR == 1 { delete prev }

        {
            stripped = $0
            sub(/^[ \t]*/, "", stripped)

            # Skip the line if it is itself a comment. Prose mentioning unsafe
            # is not unsafe, and the module docs in this tree discuss it often.
            is_comment = (stripped ~ /^(\/\/|\/\*|\*)/)

            is_block = ($0 ~ /(^|[^A-Za-z0-9_])unsafe[ \t]*\{/)
            is_impl  = ($0 ~ /(^|[^A-Za-z0-9_])unsafe[ \t]+impl([ \t<]|$)/)

            if (!is_comment && (is_block || is_impl)) {
                documented = 0
                for (i = 1; i <= window; i++) {
                    if ((FNR - i) >= 1 && prev[FNR - i] ~ /SAFETY:/) {
                        documented = 1
                        break
                    }
                }
                if (!documented) {
                    printf "%s:%d: %s\n", FILENAME, FNR, stripped
                }
            }

            prev[FNR] = $0
        }
    '
}

# Per-file counts of every `unsafe` token, for the inventory. Deliberately
# broader than the gate: a reviewer wants to see declarations too.
inventory() {
    # `-H` because grep drops the filename prefix when xargs hands it a single
    # file, which would silently turn the inventory into a bare column of counts.
    find_sources | xargs -0 -r grep -c -H -E '(^|[^A-Za-z0-9_])unsafe([^A-Za-z0-9_]|$)' \
        | awk -F: '$2 > 0 { printf "%-52s %s\n", $1, $2 }' \
        | sort -k2 -rn
}

contracts_inventory() {
    find ./contracts -type f -name '*.rs' -not -path '*/target/*' -print0 2>/dev/null \
        | sort -z \
        | xargs -0 -r grep -c -H -E '(^|[^A-Za-z0-9_])unsafe([^A-Za-z0-9_]|$)' \
        | awk -F: '$2 > 0 { printf "%-52s %s\n", $1, $2 }' \
        | sort -k2 -rn
}

summary() {
    # Present only under GitHub Actions; harmless locally.
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        cat >> "$GITHUB_STEP_SUMMARY"
    else
        cat
    fi
}

{
    echo "## First-party unsafe inventory"
    echo
    echo '```'
    inventory
    echo '```'
    echo
    echo "### Not gated: wasm contracts"
    echo
    echo "Built for wasm32 against the host ABI, outside this workspace."
    echo
    echo '```'
    contracts_inventory
    echo '```'
} | summary

undocumented="$(find_undocumented)"

if [ -n "$undocumented" ]; then
    echo
    echo "error: unsafe without a SAFETY comment:"
    echo
    echo "$undocumented" | sed 's/^/  /'
    echo
    echo "Add a '// SAFETY:' comment above each, stating the invariant that"
    echo "makes the operation sound. See benches/hybrid_footprint.rs for the"
    echo "shape this repository uses."
    exit 1
fi

echo "ok: every first-party unsafe block and impl carries a SAFETY comment"
