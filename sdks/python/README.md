# sdks/python — Maya2C for Python

```python
from maya2c_sdk import Client, Wallet

client = Client("http://127.0.0.1:8545")
wallet = Wallet("l1-wallet", "wallet.key", password, client)
txid = wallet.transfer(recipient, 1_000)
```

- `Client` — JSON-RPC: balances, the account at the tip, blocks, fee info,
  raw transaction submission. Errors are `RpcError`, never a silent `None`.
- `Wallet` — transfers built and signed by `l1-wallet`, the Rust wallet
  shipped with the node, then submitted through `Client`. The password
  reaches the child process in `L1_WALLET_PASSWORD`, never on a command line.

Standard library only.

## Why signing goes through the Rust wallet

A Maya2C signature is an ML-DSA-65 and SLH-DSA pair, and both must verify;
the transaction wire format is a consensus rule too. A second implementation
in Python would be a second thing that can disagree with the chain. The
UniFFI bindings in `sdks/sdk-ffi/bindings/python` expose the signing key for
signing messages; building transactions stays with the wallet.

## Tests

`tests/test_live_node.py` runs against a live devnet and skips without one:

```
$ cargo xtask sdk-e2e --lang python
Ran 3 tests ... OK   (a Rust-signed transfer credited in ~1 s)
```
