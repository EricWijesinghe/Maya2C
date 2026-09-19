#!/usr/bin/env bash
#
# Compile a Maya smart contract to WebAssembly.
#
# Usage:
#   ./scripts/build-contract.sh [contract-name]     default: token-swap
#   ./scripts/build-contract.sh --list
#
# Output:
#   target-contracts/wasm32-unknown-unknown/release/<name>.wasm
#
# ## Why a separate target directory
#
# Contracts build for wasm32-unknown-unknown while the node builds for the host.
# Sharing `target/` means both fight over the same cargo lock, so a build kicked
# off from inside a running `cargo test` would block until the test gave up.
# `--target-dir` keeps the two entirely independent.
#
# ## Writing a contract
#
#   #![no_std]                          no standard library, no allocator
#   crate-type = ["cdylib"]             produce a wasm module, not an rlib
#   #[panic_handler]                    required; trap rather than unwind
#   #[link(wasm_import_module = "env")] on the host-import block, or the
#                                       linker reports undefined symbols
#
# Exports the host requires:
#   input_ptr()     -> i32   address of the contract's static input buffer
#   input_cap()     -> i32   its capacity
#   invoke(len:i32) -> i64   (out_ptr << 32) | out_len ; negative on failure

set -euo pipefail

CONTRACT="${1:-token-swap}"
TARGET="wasm32-unknown-unknown"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contracts_dir="$repo_root/contracts"
target_dir="$repo_root/target-contracts"

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m  ok\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31mERROR\033[0m %s\n' "$*" >&2; exit 1; }

if [ "$CONTRACT" = "--list" ]; then
  log "available contracts"
  for dir in "$contracts_dir"/*/; do
    [ -f "$dir/Cargo.toml" ] && printf '  %s\n' "$(basename "$dir")"
  done
  exit 0
fi

command -v cargo >/dev/null 2>&1 || die "cargo is not installed"

source_dir="$contracts_dir/$CONTRACT"
[ -d "$source_dir" ] || die "no contract at $source_dir (try --list)"

# The wasm target is a rustup component, not a default. Check before building so
# the failure names the fix rather than surfacing a confusing compile error.
if command -v rustup >/dev/null 2>&1; then
  if ! rustup target list --installed | grep -qx "$TARGET"; then
    die "target $TARGET is not installed. Run: rustup target add $TARGET"
  fi
fi

log "building $CONTRACT for $TARGET"
(
  cd "$source_dir"
  cargo build --release --target "$TARGET" --target-dir "$target_dir"
)

# cargo replaces hyphens with underscores in artifact names.
artifact="$target_dir/$TARGET/release/${CONTRACT//-/_}.wasm"
[ -f "$artifact" ] || die "expected artifact not found at $artifact"

size="$(wc -c < "$artifact" | tr -d ' ')"
ok "$artifact"
ok "$size bytes"

cat <<EOF

  Deploy with:
    l1-wallet deploy --wasm $artifact

  The module is stored on chain verbatim, so its size is a recurring cost.
  Keep opt-level = "z" and lto = true in the contract's release profile.
EOF
