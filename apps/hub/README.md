# Maya2C Hub

One window for mining, the network and every Maya2C app ([ADR-045](../../docs/adr/ADR-045-maya2c-hub.md)).
Tauri 2. The UI in `ui/` is plain HTML, CSS and canvas, and the Rust commands in `src-tauri/src` do everything that touches the network, the GPU driver or a process.

| Platform | Status | How |
|---|---|---|
| Windows | Builds: NSIS `.exe` and `.msi`, unsigned | `cargo xtask hub-bundle` |
| Linux | Not built yet | `cargo xtask hub-bundle` inside WSL Ubuntu, once Rust and the WebKitGTK dev packages are installed there |
| Android | Debug APK builds and runs (emulator, API 34) | see below |
| macOS, iOS | Waiting for a Mac to build on | `cargo tauri build` / `cargo tauri ios build` |

The desktop Hub runs `maya2c-gpu-miner --mine --json` and draws its events.
On a phone the Hub only **watches**: the network, the app universe and your payouts.
It runs no miner, because a phone would spend its battery and heat for next to no hashes.

## Preview without building

`python -m http.server 8765` in `ui/`, then open it. In a plain browser the page runs on labelled preview data.

## Android

The SDK and NDK are already installed under `%LOCALAPPDATA%\Android\Sdk`. Gradle is kept off C:.

```bash
export JAVA_HOME="C:\\Program Files\\Java\\jdk-21.0.12"
export ANDROID_HOME="C:\\Users\\EricW\\AppData\\Local\\Android\\Sdk"
export NDK_HOME="$ANDROID_HOME\\ndk\\30.0.15729638"
export GRADLE_USER_HOME="D:\\Caches\\gradle"
cd apps/hub/src-tauri
cargo tauri android build --apk --target aarch64 --debug   # phones
cargo tauri android build --apk --target x86_64 --debug    # the emulator
```

A release APK or AAB for the Play Store needs an upload keystore. Generate it yourself, keep it off this repo, and never paste it into a chat.

## Notes

- Earnings shown are the payout account's **on-chain** balance growth, never the count of shares the node accepted.
- A fresh install points at the public testnet RPC (`https://rpc.maya2c.dev/rpc`). Change it in Settings.
