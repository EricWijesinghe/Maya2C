# In-app updates

Maya Wallet and Maya Chat (and the Hub, once it ships in releases) check
`https://maya2c.dev/updates/<app>.json` when they start. When a newer version
exists, a dialog offers **Install now** or **Later**. Nothing downloads
without that click. The download is checked against the update-signing
public key built into the app, then installed, and the app restarts.

| Piece | Where |
|---|---|
| Check, ask, install, restart | `apps/app-update` (shared by every app) |
| Signed update bundles | `release-binaries.yml`, *Updater config* step |
| Manifests `updates/<app>.json` | `docs/site/scripts/gen-updates.mjs`, run at every site build; the release job starts a site build |
| Public key | `apps/updater-pubkey.txt` |
| Private key | GitHub secret `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`) |

## Turning it on (once, by the owner)

Whoever holds the private key can push an update to every installed app.
So the owner generates it. It never goes into the repository, a chat or a
log.

1. On your own PC, run:

   ```powershell
   cargo install tauri-cli --locked   # skip if `cargo tauri --version` works
   cargo tauri signer generate -w D:\Keys\maya2c-updater.key
   ```

   Choose a password. This writes `maya2c-updater.key` (secret) and
   `maya2c-updater.key.pub` (public). Back up the secret one offline. **If it
   is lost, installed apps can never be updated again**, and users would have to
   reinstall by hand.

2. Give GitHub the secret, then commit the public key:

   ```powershell
   gh secret set TAURI_SIGNING_PRIVATE_KEY < D:\Keys\maya2c-updater.key
   gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD   # type the password when asked
   Copy-Item D:\Keys\maya2c-updater.key.pub apps\updater-pubkey.txt
   ```

The next `node-v*` release builds signed update bundles with app version
`0.1.<run number>`. Apps installed from that release onward update
themselves. Apps installed before it have no updater, so install those once
more by hand.

Until both steps are done, every build is the unsigned build it always was,
and the apps never offer an update.
