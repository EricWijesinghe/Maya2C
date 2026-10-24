#!/usr/bin/env bash
# Reproducible release build of maya2c-node (production features).
#
#   scripts/repro_build.sh <target-triple>
#
# Prints the SHA-256 of the binary. Two runs from the same commit, toolchain
# and target give the same hash whatever directory the checkout is in and
# whoever's home directory holds cargo: that is what lets anyone check a
# published binary against the source instead of trusting the CI that built it.
#
# The release profile is already deterministic (fat LTO, one codegen unit,
# stripped, and no build script embeds a time or a git hash). What is left is
# paths: panic messages and C++ asserts in RocksDB carry the absolute path of
# the source file. Both are mapped to fixed names here.
set -euo pipefail

target=${1:?usage: repro_build.sh <target-triple>}
src=$(pwd -P)
# Created if new: a fresh CARGO_HOME is how the CI check proves its path
# does not leak into the binary.
mkdir -p "${CARGO_HOME:-$HOME/.cargo}"
cargo_home=$(cd "${CARGO_HOME:-$HOME/.cargo}" && pwd -P)

# --config arrays are appended to .cargo/config.toml's, so the mold link-arg
# for x86_64 Linux stays; RUSTFLAGS in the environment would silently drop it.
flags="\"--remap-path-prefix=$src=/maya2c\", \"--remap-path-prefix=$cargo_home=/cargo\""
prefix_map="-ffile-prefix-map=$src=/maya2c -ffile-prefix-map=$cargo_home=/cargo"
export CFLAGS="${CFLAGS:-} $prefix_map" CXXFLAGS="${CXXFLAGS:-} $prefix_map"

cargo build --release --locked --target "$target" \
  --config "target.$target.rustflags = [$flags]" \
  -p maya2c-node --no-default-features --features production >&2

exe=target/$target/release/maya2c-node
[ -f "$exe" ] || exe=$exe.exe
sha256sum "$exe" | cut -d' ' -f1
