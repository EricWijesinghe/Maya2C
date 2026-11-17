# sdks/python — Maya2C for Python

## Install

### Not published yet, build from source

The `maya2c_sdk` package is not on PyPI (`pypi.org/pypi/maya2c-sdk` answered
404 on 2026-11-13) and has no packaging file yet. It is standard library only,
so a clone on `PYTHONPATH` is the whole install. CI runs it on Python 3.12.

```bash
git clone https://github.com/EricWijesinghe/Maya2C
export PYTHONPATH="$PWD/Maya2C/sdks/python"     # PowerShell: $env:PYTHONPATH = "$PWD\Maya2C\sdks\python"
python -c "import maya2c_sdk; print('ok')"
```

The separate signing bindings (`sdks/sdk-ffi/bindings/python`, PyPI name
`maya2c`) are not published either; see their README.

## Send a transfer

Against `cargo xtask kit up` (four validators; node 0's RPC on
`127.0.0.1:32000`), run from the repository root:

```python
import os
from maya2c_sdk import Client, RpcError, Wallet, WalletError

client = Client("http://127.0.0.1:32000")
genesis = client.call("get_chain_info")["genesis"]   # the wallet signs for this chain only (ADR-036)
wallet = Wallet(
    "target/debug/l1-wallet",          # l1-wallet.exe on Windows
    "target/up/wallet.key",
    os.environ["L1_WALLET_PASSWORD"],  # devnet: devnet-only-password (xtask/src/devnet.rs)
    client,
    genesis=genesis,
)
recipient = "7777777777777777777777777777777777777777777777777777777777777777"
try:
    txid = wallet.transfer(recipient, 1_000)
    print("accepted into the mempool:", txid)
    client.wait_until(lambda: client.balance(recipient) >= 1_000, timeout=60)
    print("credited:", client.balance(recipient))
except (RpcError, WalletError, TimeoutError) as error:
    print("refused:", error)
```

- `Client` — JSON-RPC: balances, the account at the tip, blocks, fee info,
  raw transaction submission (`client.call(method, params)` for anything else).
  Errors are `RpcError`, never a silent `None`.
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

## What is checked

- `.github/workflows/sdks.yml` runs `python -m unittest discover -s tests` on
  every change under `sdks/`; `tests/test_live_node.py` skips there.
- `cargo xtask sdk-e2e --lang python` runs `tests/test_live_node.py` against a
  one-validator devnet: reads, a transfer signed by `l1-wallet` and submitted
  with `Wallet.transfer` (the flow above), and a refusal. The nightly CI job
  runs it.

```
$ cargo xtask sdk-e2e --lang python
Ran 3 tests ... OK   (a Rust-signed transfer credited in ~1 s)
```
