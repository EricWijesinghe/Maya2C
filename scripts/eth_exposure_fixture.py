#!/usr/bin/env python3
"""Fetch a real Ethereum mainnet sample for the quantum-exposure tool (MP28).

Takes the last N blocks from a public JSON-RPC endpoint, collects every
sender and recipient, and records for each address its nonce, whether it has
code, and its balance at one pinned block. Also records one raw signed
transaction with its hash and sender, so the Rust test can recover the
sender's secp256k1 public key from the signature and show the key really is
on chain.

Writes crates/quantum-harbor/tests/fixtures/eth_mainnet_sample.json. The
fixture is data, dated and pinned to a block; the test never touches the
network.

    python scripts/eth_exposure_fixture.py [--rpc URL] [--blocks 3]
"""

from __future__ import annotations

import argparse
import datetime
import json
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "crates/quantum-harbor/tests/fixtures/eth_mainnet_sample.json"


def call(rpc: str, method: str, *params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": list(params)}).encode()
    req = urllib.request.Request(rpc, data=body, headers={"content-type": "application/json", "user-agent": "maya2c-exposure-fixture/1"})
    with urllib.request.urlopen(req, timeout=30) as r:
        reply = json.loads(r.read())
    if "error" in reply:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", default="https://ethereum-rpc.publicnode.com")
    ap.add_argument("--blocks", type=int, default=3)
    ap.add_argument("--max-accounts", type=int, default=300)
    args = ap.parse_args()

    head = int(call(args.rpc, "eth_blockNumber"), 16)
    pinned = hex(head)
    addresses: list[str] = []
    raw = None
    for n in range(head - args.blocks + 1, head + 1):
        block = call(args.rpc, "eth_getBlockByNumber", hex(n), True)
        for tx in block["transactions"]:
            for a in (tx["from"], tx.get("to")):
                if a and a not in addresses:
                    addresses.append(a)
            if raw is None and tx.get("type") == "0x2":
                raw = {
                    "hash": tx["hash"],
                    "from": tx["from"],
                    "raw": call(args.rpc, "eth_getRawTransactionByHash", tx["hash"]),
                }
    addresses = addresses[: args.max_accounts]
    accounts = []
    for a in addresses:
        accounts.append({
            "address": a,
            "nonce": int(call(args.rpc, "eth_getTransactionCount", a, pinned), 16),
            # The first three bytes are enough to tell a contract from an
            # EIP-7702-delegated EOA (0xef0100...), which still has a key.
            "code_prefix": call(args.rpc, "eth_getCode", a, pinned)[:8],
            "balance_wei": str(int(call(args.rpc, "eth_getBalance", a, pinned), 16)),
        })
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({
        "source": args.rpc,
        "fetched_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "pinned_block": head,
        "blocks_sampled": args.blocks,
        "signed_transaction": raw,
        "accounts": accounts,
    }, indent=1))
    print(f"{len(accounts)} accounts at block {head} -> {OUT}")


if __name__ == "__main__":
    main()
