# Maya2C Wallet screenshots

Taken by the wallet's end-to-end suite (`apps/wallet-gui/e2e/tests/wallet_flows.rs`,
`MAYA_E2E_SHOTS`) from the release build in WebView2, at 360, 768 and 1440 CSS px
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
