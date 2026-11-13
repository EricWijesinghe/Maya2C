#!/usr/bin/env bash
# Builds sdks/sdk-wasm for the browser into apps/wallet-extension/wasm/.
# wasm-bindgen-cli must match the wasm-bindgen crate in Cargo.lock exactly.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$here/../.."
want="$(grep -A1 '^name = "wasm-bindgen"$' "$root/Cargo.lock" | sed -n 's/^version = "\(.*\)"/\1/p')"
have="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')"
if [ "$want" != "$have" ]; then
  echo "wasm-bindgen-cli $have does not match the crate's $want: cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi
(cd "$root" && cargo build -p maya-sdk-wasm --target wasm32-unknown-unknown --release)
target_dir="${CARGO_TARGET_DIR:-$root/target}"
wasm-bindgen --target web --out-dir "$here/wasm" --out-name maya_sdk_wasm \
  "$target_dir/wasm32-unknown-unknown/release/maya_sdk_wasm.wasm"
