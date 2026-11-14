# App link schemes (ADR-083 decision 4)

The Hub opens the Wallet and Chat apps through OS link schemes. The OS starts
the installed app; the Hub never resolves or runs a binary path, and it holds
no keys.

| App | Scheme | Routes accepted |
|---|---|---|
| Maya2C Wallet (`apps/wallet-gui`) | `maya2c-wallet://` | `open`; `pay?to=<maya1…>&amount=<decimal>` (amount optional) |
| Maya Chat (`apps/chat-gui`) | `maya2c-chat://` | `open` |

Anything else is refused: other schemes, other routes, unknown or repeated
query keys, percent-encoding, fragments, non-ASCII, links over 512 bytes,
and addresses that fail the bech32m checksum. Refusals are logged as the
reason only, never the input.

## Wallet `pay` link

- `to` is a bech32m address (`crates/address`). Lowercase only. Any known
  network prefix is accepted; the Send screen's preview still shows the
  chain before anything is signed.
- `amount` is a whole-coin decimal (`1.5`), parsed exactly by the wallet
  core's `parse_display` (24 decimal places, ADR-068). Zero, signs,
  exponents and more than 24 fractional digits are refused. Note this differs
  from `maya:` QR URIs, which carry base units.
- The link only fills the Send form. The app takes the pending request on
  the Wallet screen only, so a link never interrupts an unlock, backup or
  signing step. Signing still needs the unlocked session and the native
  confirm dialog.

## Hub

`capabilities/default.json` grants `opener:allow-open-url` for exactly
`maya2c-wallet://open` and `maya2c-chat://open`. No other URL can be opened
from the webview. If the scheme has no handler, the Hub shows the install
message instead of failing silently.

## Registration by installer

Each app declares its schemes in `tauri.conf.json`:

```json
"plugins": { "deep-link": { "desktop": { "schemes": ["maya2c-wallet"] } } }
```

- **Windows NSIS and MSI:** the bundler registers the scheme at install.
  The plugin's uninstall path clears both `HKCU` and `HKLM` keys
  (`Software\Classes\<scheme>`), which covers a per-machine NSIS install.
  The app does not write registry keys at runtime. A runtime write from
  whichever `.exe` happens to run would let a copied build take the scheme.
- **Linux `.desktop`:** the bundler is expected to add
  `MimeType=x-scheme-handler/<scheme>;` to the generated desktop entry.
  Confirm this on a built package (see below).
- **macOS:** the scheme goes into `Info.plist` (`CFBundleURLTypes`) at
  bundle time. Platform targets say macOS comes later.
- **Second launch:** `tauri-plugin-single-instance` (deep-link feature) hands
  the second launch's link to the running app, which focuses its window.

## Not yet verified (needs an installed build)

1. Windows NSIS and MSI: `HKCU\Software\Classes\maya2c-wallet` exists after
   install, and `maya2c-wallet://open` starts or focuses the app.
2. Linux `.deb`/AppImage: the desktop entry has the `MimeType` line, and
   `xdg-open maya2c-wallet://open` starts the app.
3. The Hub's Open buttons on a machine with the apps installed and on one
   without (install message).
4. The Wallet `pay` link prefills Send with the right address and amount, and
   does not sign.

Record each run in `reports/` with the command and output before marking the
catalog row (`hub-launch`) done.
