#!/bin/bash -eu
# Builds every cargo-fuzz target in fuzz/fuzz_targets into $OUT.
cd "$SRC/maya2c"

# The repository root pins nightly-2026-07-15 in rust-toolchain.toml. OSS-Fuzz
# builds with the nightly in its base image, and rustup would otherwise try to
# download the pinned one. This is a throwaway checkout, not a source change.
rm -f rust-toolchain.toml

cd fuzz
cargo fuzz build -O --debug-assertions

FUZZ_TARGET_OUTPUT_DIR=target/x86_64-unknown-linux-gnu/release
for f in fuzz_targets/*.rs; do
    FUZZ_TARGET_NAME=$(basename "${f%.*}")
    cp "$FUZZ_TARGET_OUTPUT_DIR/$FUZZ_TARGET_NAME" "$OUT/"
done
