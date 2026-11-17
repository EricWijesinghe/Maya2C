# sdks/go — Maya2C for Go

## Install

```bash
go get maya2c.dev/sdk
```

Needs **Go 1.25 or later** to fetch. `maya2c.dev/sdk` is served by a
`go-import` tag on the website (`docs/site/public/sdk/index.html`) that names
this subdirectory of the repository, and only Go 1.25+ reads that field. The
module itself still builds with the Go version in `go.mod`. There is no tagged
release: `go get` resolves a pseudo-version of master (checked against
`proxy.golang.org` on 2026-11-13).

Signing needs `l1-wallet`, which is not published as a binary package: build it
from a clone (`cargo build -p l1-wallet`, or `cargo xtask kit up`, which builds
it into `target/debug/`).

## Send a transfer

Against `cargo xtask kit up` (four validators; node 0's RPC on
`127.0.0.1:32000`), with `L1_WALLET`, `MAYA_KEYSTORE` and `L1_WALLET_PASSWORD`
set as in the comment (the names the live test reads), then `go run .` in your
module:

```go
package main

import (
	"fmt"
	"log"
	"math/big"
	"os"

	maya2c "maya2c.dev/sdk"
)

func main() {
	client := maya2c.NewClient("http://127.0.0.1:32000")
	var info struct{ Genesis string }
	if err := client.Call("get_chain_info", nil, &info); err != nil {
		log.Fatal(err)
	}
	// L1_WALLET: <repo>/target/debug/l1-wallet (.exe on Windows); MAYA_KEYSTORE:
	// <repo>/target/up/wallet.key; devnet password devnet-only-password (xtask/src/devnet.rs).
	wallet := maya2c.NewWallet(os.Getenv("L1_WALLET"), os.Getenv("MAYA_KEYSTORE"),
		os.Getenv("L1_WALLET_PASSWORD"), client)
	wallet.Genesis = info.Genesis // the wallet signs for this chain only (ADR-036)

	recipient := "7777777777777777777777777777777777777777777777777777777777777777"
	txid, err := wallet.Transfer(recipient, big.NewInt(1_000)) // amounts are 256-bit (ADR-046)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("accepted into the mempool:", txid)
	balance, err := client.Balance(recipient)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("recipient balance:", balance) // poll: acceptance is not inclusion
}
```

- `Client` — JSON-RPC: balances, the account at the tip (read with the tip's
  height and id, for reconciliation), blocks, raw transaction submission, and
  `Call` for any other method.
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

## What is checked

- `.github/workflows/sdks.yml` runs `go test ./...` on every change under
  `sdks/`; the live-node tests skip there. The same workflow resolves the
  `go get` line above against `proxy.golang.org`.
- `cargo xtask sdk-e2e --lang go` runs `live_node_test.go` against a
  one-validator devnet: reads, a transfer signed by `l1-wallet` and submitted
  with `Wallet.Transfer` (the flow above), and a refusal. The nightly CI job
  runs it.

```
$ GO=path/to/go cargo xtask sdk-e2e --lang go
--- PASS: TestReadsTheChain
--- PASS: TestATransferSignedInRustIsCredited   (credited in ~2 s)
--- PASS: TestARefusalIsAnError
```
