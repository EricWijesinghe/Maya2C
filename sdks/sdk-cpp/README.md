# Maya2C C++ SDK (`sdks/sdk-cpp`)

Header-only C++20 over [`sdk-ffi`](../sdk-ffi), the same Rust library the
Python, Kotlin and Swift bindings load. Status: **stub** in `features.toml`;
the first compile and test run is the `C++ SDK tests` job in
`.github/workflows/sdks.yml` (Ubuntu only).

What it gives you:

| Header | What |
|---|---|
| `maya2c/keys.hpp` | `SigningKey` (owning handle, move-only), `Seed` (wiped on destruction), `verify`, `account_id_from_public_key` |
| `maya2c/address.hpp` | `Address`: Bech32m text (ADR-081), `maya` / `tmaya` / `dmaya`, checked against `spec/address-vectors.json` |
| `maya2c/amount.hpp` | `Amount`: 256-bit base units, exactly 24 decimals, no floats; a 25th decimal is refused, never rounded |
| `maya2c/result.hpp` | `Result<T>` and `Error` / `ErrorCode`; no exceptions cross the C boundary |
| `maya2c/sdk.hpp` | all of the above |

```cpp
#include "maya2c/sdk.hpp"

auto key = maya2c::SigningKey::generate();
if (!key) return fail(key.error().message);
auto address = key.value().address(maya2c::Network::Testnet);  // tmaya1...
auto signature = key.value().sign(message);                     // 11,165 bytes
auto ok = maya2c::verify(key.value().public_key().value(), message, signature.value());
auto amount = maya2c::Amount::parse_coins("1.5");               // 1.5 * 10^24 base units
```

## The secret never crosses

As in `sdk-ffi`: the secret key lives in Rust, is zeroized there when the
handle is freed, and there is no call that exports it. `SigningKey`'s
destructor calls `uniffi_maya_sdk_ffi_fn_free_signingkey`; copy is deleted.
The one secret this side holds is a `Seed` you brought; it is wiped with
`secure_zero` (glibc `explicit_bzero`, else a volatile loop) on destruction and
when moved from, and the bytes handed to Rust for `from_seed` are wiped before
the scratch buffer is freed.

Known limit: Rust receives the seed as a `Vec<u8>` and drops it without
zeroizing (that is in `sdk-ffi`, not here).

## Not here yet

`sdk-ffi` exports keys, signing, verification and the account id. It has no
transaction builder and no RPC client, so neither does this SDK. Building and
submitting a transfer needs those added to `sdk-ffi` first (or use the Rust
SDK in `sdks/rust`).

## Build

Build `sdk-ffi` from the repo root first. The dev profile is enough (every
dependency is already optimised in dev) and avoids the release profile's fat LTO.

### Linux

```sh
sudo apt-get install -y clang libclang-dev mold cmake libgtest-dev
cargo build -p maya-sdk-ffi --lib
cmake -S sdks/sdk-cpp -B build/sdk-cpp -DMAYA_SDK_FFI_DIR="$PWD/target/debug"
cmake --build build/sdk-cpp -j 4
ctest --test-dir build/sdk-cpp --output-on-failure
```

### macOS (not yet run)

```sh
brew install cmake googletest
cargo build -p maya-sdk-ffi --lib
cmake -S sdks/sdk-cpp -B build/sdk-cpp -DMAYA_SDK_FFI_DIR="$PWD/target/debug"
cmake --build build/sdk-cpp -j 4
ctest --test-dir build/sdk-cpp --output-on-failure
```

### Windows (not yet run)

From a Developer PowerShell for Visual Studio (MSVC 2022 or newer):

```powershell
cargo build -p maya-sdk-ffi --lib
cmake -S sdks/sdk-cpp -B build/sdk-cpp -DMAYA_SDK_FFI_DIR="$PWD/target/debug"
cmake --build build/sdk-cpp --config Debug
ctest --test-dir build/sdk-cpp -C Debug --output-on-failure
```

CMake links `maya_sdk_ffi.dll.lib` and ctest puts the DLL's directory on
`PATH`. To run the example by hand, copy `target/debug/maya_sdk_ffi.dll` next to it.

If GoogleTest is not installed, CMake fetches v1.15.2 from GitHub.

## Using it from your own CMake project

```cmake
set(MAYA_SDK_FFI_DIR "/path/to/target/debug")
add_subdirectory(path/to/sdks/sdk-cpp)
target_link_libraries(your_app PRIVATE maya2c::sdk)
```

Ship `libmaya_sdk_ffi.so` / `.dylib` / `maya_sdk_ffi.dll` with your binary.

## How it talks to Rust

`sdk-ffi` is a uniffi crate, so its C ABI is uniffi's scaffolding (contract
version 30). `include/maya2c/detail/ffi.hpp` re-declares the functions it calls,
because the generated header (`sdks/sdk-ffi/bindings/swift/maya_sdk_ffiFFI.h`)
uses clang-only nullability keywords. Do not include both in one translation
unit. The calls check `ffi_maya_sdk_ffi_uniffi_contract_version()` before
anything else and fail with `ErrorCode::Internal` on a mismatch. The
per-function uniffi checksums are not checked yet (the generated Python leaves
them off too).
