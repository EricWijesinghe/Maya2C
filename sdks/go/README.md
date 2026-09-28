# sdks/go — Maya2C for Go

```bash
go get maya2c.dev/sdk
```

Needs **Go 1.25 or later** to fetch. `maya2c.dev/sdk` is served by a
`go-import` tag on the website (`docs/site/public/sdk/index.html`) that names
this subdirectory of the repository, and only Go 1.25+ reads that field. The
module itself still builds with the Go version in `go.mod`.

```go
import maya2c "maya2c.dev/sdk"

client := maya2c.NewClient("http://127.0.0.1:8545")
wallet := maya2c.NewWallet("l1-wallet", "wallet.key", password, client)
txid, err := wallet.Transfer(recipient, 1_000)
```

- `Client` — JSON-RPC: balances, the account at the tip (read with the tip's
  height and id, for reconciliation), blocks, raw transaction submission.
- `Wallet` — transfers built and signed by `l1-wallet`, the Rust wallet
  shipped with the node, then submitted through `Client`. The password
  reaches the child process in `L1_WALLET_PASSWORD`, never on a command line.

Standard library only; no cgo.

## Why signing goes through the Rust wallet

A Maya2C signature is an ML-DSA-65 and SLH-DSA pair, and both must verify;
the transaction wire format is a consensus rule too. A second implementation
in Go would be a second thing that can disagree with the chain
(`docs/invariants.md`, invariant 2). So this package never signs or encodes
a transaction itself: it asks the one implementation consensus uses.

## Tests

`live_node_test.go` runs against a live devnet and skips without one:

```
$ GO=path/to/go cargo xtask sdk-e2e --lang go
--- PASS: TestReadsTheChain
--- PASS: TestATransferSignedInRustIsCredited   (credited in ~2 s)
--- PASS: TestARefusalIsAnError
```
