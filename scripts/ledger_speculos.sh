#!/usr/bin/env bash
# Builds the Ledger app for Nano S Plus and runs tests/ledger_tests.rs against
# it under the Speculos emulator. Linux only (WSL works): Speculos runs the
# ARM binary with qemu-arm-static.
#
# Needs, all installed by the steps in docs/ledger-feasibility.md:
#   arm-none-eabi-gcc, clang, qemu-arm-static   (apt)
#   speculos, ledgerblue                        (pip, in $SPECULOS_VENV)
#   cargo-ledger                                (cargo install --locked)
#   a Ledger C SDK checkout                     ($LEDGER_SDK_PATH)
#
# Usage: [DEVICE=nanosplus|nanox] scripts/ledger_speculos.sh [app-dir]
# (Stax and Flex are touch screens; the tests drive buttons.)
set -euo pipefail

APP_DIR=${1:-$(cd "$(dirname "$0")/../apps/ledger-maya2c" && pwd)}
TOOLCHAIN=${TOOLCHAIN:-nightly-2026-07-15}
DEVICE=${DEVICE:-nanosplus}
case "$DEVICE" in
    nanosplus) MODEL=nanosp ;;
    nanox) MODEL=nanox ;;
    *) echo "button-driven tests cover nanosplus and nanox, not $DEVICE" >&2; exit 2 ;;
esac
SPECULOS_VENV=${SPECULOS_VENV:-/root/speculos-venv}
: "${LEDGER_SDK_PATH:?set LEDGER_SDK_PATH to a ledger-secure-sdk checkout}"
export LEDGER_SDK_PATH
# cargo-ledger calls `python3 -m ledgerblue.loadApp` to write the .apdu
# install file; ledgerblue lives in the same venv as Speculos.
export PATH="$SPECULOS_VENV/bin:$PATH"
# Custom target JSONs need this on current nightlies.
export RUSTFLAGS="-Zunstable-options"
# The Speculos seed. Must equal `MNEMONIC` in tests/ledger_tests.rs.
MNEMONIC="abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"

cd "$APP_DIR"
cargo +"$TOOLCHAIN" ledger setup >/dev/null
cargo +"$TOOLCHAIN" ledger build "$DEVICE" -- \
    -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem
ELF=target/$DEVICE/release/app-maya2c
arm-none-eabi-size "$ELF"

"$SPECULOS_VENV/bin/speculos" --model "$MODEL" --display headless \
    --seed "$MNEMONIC" --apdu-port 9999 --api-port 5000 "$ELF" \
    >target/speculos.log 2>&1 &
SPECULOS=$!
trap 'kill $SPECULOS 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
    (echo >/dev/tcp/127.0.0.1/9999) 2>/dev/null && break
    sleep 1
done

# Host-target tests; RUSTFLAGS is for the device build only.
unset RUSTFLAGS
cargo +"$TOOLCHAIN" test --release --test ledger_tests -- --ignored --test-threads=1
