# In-app updates

**Manual only, until update manifests are k-of-n signed** (ADR-080,
amendment 2026-11-14). The apps never check on start. One key signs the
updates, and an automatic check would carry a stolen key's update to every
install.

Maya Wallet (network settings), Maya Chat (settings) and the Hub (settings)
each have a **Check for updates** button. It asks
`https://maya2c.dev/updates/<app>.json` whether a newer version exists. When
one does, a dialog names it, shows the **publisher key fingerprint**
(SHA-256 of the update public key) and offers **Install now** or **Later**.
Nothing downloads without that click. The download must be signed by the
public key built into the app; an unsigned or wrongly signed one is refused.
Then it installs and the app restarts.

To check the fingerprint yourself from the public key file:

```bash
base64 -d apps/updater-pubkey.txt | sed -n 2p | base64 -d | sha256sum
```

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
`0.1.<run number>`. Apps installed from that release onward can update
from **Check for updates**. Apps installed before it have no updater, so
install those once more by hand.

3. Publish the fingerprint (the command above) in `docs/verify-release.md`,
   so users can compare it with the one the update dialog shows.

Until both steps are done, every build is the unsigned build it always was,
and the apps never offer an update.
