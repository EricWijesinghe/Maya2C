# Maya2C Wallet screenshots

Taken by the wallet's end-to-end suite (`apps/wallet-gui/e2e/tests/wallet_flows.rs`,
`MAYA_E2E_SHOTS`) from the release build in WebView2, at 360 and 1440 CSS px
wide and 740 px tall, in the dark and light themes.

| Files | Node behind the screen |
|---|---|
| `welcome-*`, `setup-*`, `backup-*`, `backup-check-*` | none (no wallet yet) |
| `home-default-node-*` | the default public endpoint; with the testnet paused it does not answer, so the screen shows the paused card and "Balance unavailable" |
| `send-review-stub-*`, `account-stub-*` | **the e2e stub node** (`apps/wallet-gui/e2e/src/stub_node.rs`) |

The stub node is a fixture, not a network: it reports the network name
`maya-e2e-stub`, height 4,242, and 1,000 MAYA for every address, and refuses
anything but those reads. Every `-stub` picture carries a caption saying so
across its top. None of these figures comes from a live network, and mainnet
is not live.

Colour: the suite starts WebView2 with `--force-color-profile=srgb`, so each
PNG holds the sRGB values the tokens are written in. Pictures taken without
it (before this note) were captured in the display's wide-gamut profile and
saved untagged, which made `#3bc9b5` read as about `rgb(102,193,176)`. The
suite now samples the primary button's painted pixel from a real screenshot
and fails if it is more than 8 levels from its role's colour.
