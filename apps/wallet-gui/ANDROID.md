# Maya Wallet on Android (P-10)

Status: **skeleton** (features.toml `android-wallet`, `stub`). The project builds the same
Rust core and Leptos screens as the desktop wallet. It cannot hold a key yet: see
[Key storage](#key-storage-todo) below. Nothing here mines, and nothing ever will: phones
watch and sign, they do not mine (platform rules, 2026-10-12).

## What is in the tree

| Path | What |
|---|---|
| `src-tauri/gen/android/` | Tauri 2 Android Studio project from `cargo tauri android init` (tauri-cli 2.11.4), with three edits listed below |
| `src-tauri/src/lib.rs` | `run()` is already the `mobile_entry_point`; the single-instance plugin is desktop-only (`#[cfg(desktop)]`) |
| `src-tauri/Cargo.toml` | `tauri-plugin-single-instance` moved under a not-Android, not-iOS target table: the crate is empty on mobile |
| `src-tauri/src/commands.rs`, `deeplink.rs` | the confirm dialog's `parent` window and `unminimize` are desktop-only APIs, now `#[cfg(desktop)]` |
| `ui/styles.css` | phone-width rules (360 px and up), safe-area padding for the edge-to-edge activity |
| `ui/tests/overflow.mjs` | Playwright check: every screen at 360 and 390 px, no horizontal overflow |

Edits to the generated project, each for a reason:

- `AndroidManifest.xml`: `allowBackup="false"` and `fullBackupContent="false"`. Wallet data
  does not belong in `adb backup` or a cloud backup.
- `AndroidManifest.xml`: `CAMERA` permission with `camera.any` not required, for the Scan
  screen. Android asks at runtime; a phone without a camera can still install.
- `MainActivity.kt`: `FLAG_SECURE`. No screenshots, screen recording or recents thumbnail
  of the recovery phrase.

## Build

The SDK and NDK are installed per user under `%LOCALAPPDATA%\Android\Sdk`. This machine
also runs an emulator for another project: these steps build, they never start, stop or
install to an emulator or `adb` device.

```bash
export JAVA_HOME="C:\\Program Files\\Java\\jdk-21.0.12"
export ANDROID_HOME="$LOCALAPPDATA\\Android\\Sdk"
export NDK_HOME="$ANDROID_HOME\\ndk\\30.0.15729638"
export GRADLE_USER_HOME="D:\\Caches\\gradle"          # keep Gradle off C:
rustup target add aarch64-linux-android x86_64-linux-android
cd apps/wallet-gui/src-tauri
cargo tauri android build --apk --target aarch64 --debug   # phones
cargo tauri android build --apk --target x86_64 --debug    # an emulator
```

The debug APK lands under `gen/android/app/build/outputs/apk/`. Install it with
`adb install` on a device you choose.

The wallet core links the node crate, so the first Android build also cross-compiles
RocksDB with the NDK's clang. Expect it to take a while.

### Type-check only, without Gradle

`cargo tauri` sets the NDK compilers itself. A bare `cargo check` needs them named, and
RocksDB's bindgen needs a *versioned* triple (the NDK headers refuse
`aarch64-linux-android` without an API level):

```bash
n="$NDK_HOME/toolchains/llvm/prebuilt/windows-x86_64"
export CC_aarch64_linux_android="$n/bin/aarch64-linux-android24-clang.cmd"
export CXX_aarch64_linux_android="$n/bin/aarch64-linux-android24-clang++.cmd"
export AR_aarch64_linux_android="$n/bin/llvm-ar.exe"
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android="--target=aarch64-linux-android24 --sysroot=$n/sysroot"
cd apps/wallet-gui/src-tauri      # needs ../ui/dist: run trunk build first
cargo check --target aarch64-linux-android
```

### Release signing

Every shipped build is signed (platform rules). A Play Store AAB or a release APK needs
an upload keystore: generate it yourself with `keytool`, keep it off this repo and off
this machine's shared folders, and point `gen/android/keystore.properties` at it (that
file is gitignored). Never paste it into a chat or a CI log.

## Phone layout

The screens are the desktop ones; `styles.css` makes them fit a phone. Check with:

```bash
cd apps/wallet-gui
trunk build --config ui/Trunk.toml
npm install --no-save --prefix ui/tests playwright@1.56.1
node ui/tests/overflow.mjs
```

It fakes the Tauri bridge with worst-case data (a 24-word phrase, full-length addresses,
a 30-digit base-unit balance), so it needs no core, keychain or node. The Scan screen is checked
without a camera.

## Key storage (TODO)

**Today an Android build cannot create or unlock a wallet, by design of what it links,
and it never writes a key to a plain file.**

The desktop wallet stores one secret: the seed sealed under an Argon2id passphrase key
(ChaCha20-Poly1305), in the OS keychain through `keyring` 4. keyring 4's default `v1`
API selects a store only on Windows, macOS and Linux; on Android `Entry::new` returns
`Invalid("platform")`. So `OsKeychain::put` fails and setup shows the error. That is
the intended fail-closed state until the work below lands.

### Threat model

| Threat | Without a Keystore-bound store | With it |
|---|---|---|
| Another app reads the wallet's files | Android's per-app sandbox only | sealed blob is useless without a key that never leaves the Keystore |
| Rooted phone, offline copy of `/data/data/com.maya2c.wallet` | attacker gets the sealed seed and can brute-force the passphrase offline (Argon2id slows, does not stop) | blob is bound to this device's Keystore key; an offline copy cannot be opened elsewhere |
| `adb backup`, cloud backup | blocked by `allowBackup="false"` | same |
| Screenshot, screen recording, recents thumbnail of the phrase | blocked by `FLAG_SECURE` | same |
| Malware with the user's unlocked phone | out of scope for storage; the confirm screen and passphrase are the defence | same |
| Lost phone | the passphrase alone protects the seed | Keystore key can require user authentication (biometric or screen lock) per use |

### Plan

1. Implement `SecretStore` (core `vault.rs`) for Android over the Android Keystore: an
   AES-256-GCM key generated inside the Keystore (StrongBox when the device has one,
   `setUserAuthenticationRequired` optional), wrapping the already-sealed seed, the blob
   in app-private storage. Candidates: keyring's `android-native-keyring-store` (needs a
   `cargo vet` entry and a review of how it reaches the JNI context), or a small Tauri
   plugin in Kotlin calling `KeyStore` directly.
2. Keep the passphrase layer: the Keystore wraps, it does not replace the Argon2id seal.
3. A device test that creates, locks, unlocks and deletes a wallet, run on a phone the
   owner chooses, before `android-wallet` leaves `stub`.

## Not done yet

- Keystore storage (above).
- `maya2c-wallet://` links on Android: the deep-link config has `desktop` schemes only.
- In-app updates on Android: Play Store or a signed APK channel, not the desktop updater.
- An APK built and run on a real phone.
