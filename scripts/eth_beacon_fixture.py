#!/usr/bin/env python3
"""Fetch a real Ethereum beacon light-client proof for the interop tests (MP25).

Saves, from a public beacon API and a public execution RPC:
- the latest light-client finality update (attested + finalized headers,
  finality branch, sync aggregate, signature slot);
- the light-client bootstrap for the finalized block root (the current sync
  committee and its branch into that header's state root);
- the execution header the finalized beacon block commits to, by hash.

Writes crates/interop/tests/fixtures/eth_beacon_finality.json. The Rust test
verifies it offline: committee branch, BLS aggregate signature, finality
branch, execution branch, and the execution header's Keccak hash.

    python scripts/eth_beacon_fixture.py
"""

from __future__ import annotations

import datetime
import json
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "crates/interop/tests/fixtures/eth_beacon_finality.json"
BEACON = "https://ethereum-beacon-api.publicnode.com"
RPC = "https://ethereum-rpc.publicnode.com"
UA = {"user-agent": "maya2c-beacon-fixture/1"}


def get(path: str):
    req = urllib.request.Request(BEACON + path, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())


def rpc(method: str, *params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": list(params)}).encode()
    req = urllib.request.Request(RPC, data=body, headers={"content-type": "application/json", **UA})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())["result"]


def header_root(beacon: dict) -> str:
    """The finalized header's block root, from the API rather than computed:
    the test computes it and must agree."""
    slot = beacon["slot"]
    return get(f"/eth/v1/beacon/headers/{slot}")["data"]["root"]


def main() -> None:
    update = get("/eth/v1/beacon/light_client/finality_update")
    finalized = update["data"]["finalized_header"]["beacon"]
    root = header_root(finalized)
    bootstrap = get(f"/eth/v1/beacon/light_client/bootstrap/{root}")
    genesis = get("/eth/v1/beacon/genesis")["data"]
    fork = get("/eth/v1/beacon/states/head/fork")["data"]
    block_hash = update["data"]["finalized_header"]["execution"]["block_hash"]
    execution = rpc("eth_getBlockByHash", block_hash, False)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({
        "source": {"beacon": BEACON, "execution": RPC},
        "fetched_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "genesis_validators_root": genesis["genesis_validators_root"],
        "fork": fork,
        "finalized_root_from_api": root,
        "finality_update": update,
        "bootstrap": bootstrap,
        "execution_header": execution,
    }, indent=1))
    print(f"finalized slot {finalized['slot']} root {root} exec {block_hash} -> {OUT}")


if __name__ == "__main__":
    main()
