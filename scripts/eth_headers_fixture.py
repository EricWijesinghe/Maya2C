#!/usr/bin/env python3
"""Fetch consecutive real Ethereum mainnet headers for the interop tests (MP25).

Writes crates/interop/tests/fixtures/eth_mainnet_headers.json: the last N
headers as the node returned them (every field the hasher needs, hex), with
the endpoint and the fetch time. The Rust test recomputes each block hash
from its fields and checks the parent links; it never touches the network.

    python scripts/eth_headers_fixture.py [--rpc URL] [--count 64]
"""

from __future__ import annotations

import argparse
import datetime
import json
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "crates/interop/tests/fixtures/eth_mainnet_headers.json"
FIELDS = [
    "hash", "parentHash", "sha3Uncles", "miner", "stateRoot", "transactionsRoot",
    "receiptsRoot", "logsBloom", "difficulty", "number", "gasLimit", "gasUsed",
    "timestamp", "extraData", "mixHash", "nonce", "baseFeePerGas", "withdrawalsRoot",
    "blobGasUsed", "excessBlobGas", "parentBeaconBlockRoot", "requestsHash",
]


def call(rpc: str, method: str, *params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": list(params)}).encode()
    req = urllib.request.Request(
        rpc, data=body,
        headers={"content-type": "application/json", "user-agent": "maya2c-headers-fixture/1"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        reply = json.loads(r.read())
    if "error" in reply:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", default="https://ethereum-rpc.publicnode.com")
    ap.add_argument("--count", type=int, default=64)
    args = ap.parse_args()
    head = int(call(args.rpc, "eth_blockNumber"), 16)
    headers = []
    for n in range(head - args.count + 1, head + 1):
        block = call(args.rpc, "eth_getBlockByNumber", hex(n), False)
        headers.append({k: block[k] for k in FIELDS if k in block})
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({
        "source": args.rpc,
        "fetched_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "headers": headers,
    }, indent=1))
    print(f"{len(headers)} headers {head - args.count + 1}..{head} -> {OUT}")


if __name__ == "__main__":
    main()
