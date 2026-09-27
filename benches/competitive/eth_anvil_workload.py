#!/usr/bin/env python3
"""End-to-end transfer workload against Foundry's anvil (Master Prompt 21 §4).

Starts `anvil --block-time 1` with enough funded accounts, pre-signs N ETH
transfers (secp256k1, EIP-1559) across them, submits all over JSON-RPC, and
measures wall time from the first submission until every transaction has a
receipt. Prints one line the harness records.

What this compares against, honestly: anvil is a single-process development
node — no consensus, no peers, no fsync'd database. Maya2C's number beside it
(`examples/bft_tps.rs`) is four BFT validators in one process, each verifying
post-quantum signatures and committing to RocksDB. The two rows share a
workload shape, not a threat model.

    python3 benches/competitive/eth_anvil_workload.py [transfers] [accounts]
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
import urllib.request

from eth_account import Account

PORT = 8645
MNEMONIC = "test test test test test test test test test test test junk"


def rpc(method: str, params: list, batch: list | None = None):
    body = batch if batch is not None else {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
    req = urllib.request.Request(
        f"http://127.0.0.1:{PORT}", data=json.dumps(body).encode(),
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())


def main() -> None:
    transfers = int(sys.argv[1]) if len(sys.argv) > 1 else 2000
    accounts = int(sys.argv[2]) if len(sys.argv) > 2 else 200
    anvil = subprocess.Popen(
        ["anvil", "--port", str(PORT), "--block-time", "1", "--accounts", str(accounts),
         "--mnemonic", MNEMONIC, "--silent"],
    )
    try:
        for _ in range(100):
            try:
                rpc("eth_chainId", [])
                break
            except Exception:  # noqa: BLE001 - not up yet
                time.sleep(0.1)
        chain_id = int(rpc("eth_chainId", [])["result"], 16)
        version = subprocess.run(["anvil", "--version"], capture_output=True, text=True).stdout.splitlines()[0]
        Account.enable_unaudited_hdwallet_features()
        keys = [Account.from_mnemonic(MNEMONIC, account_path=f"m/44'/60'/0'/0/{i}") for i in range(accounts)]
        base_fee = int(rpc("eth_getBlockByNumber", ["latest", False])["result"]["baseFeePerGas"], 16)
        signed = []
        for n in range(transfers):
            k = keys[n % accounts]
            tx = {
                "chainId": chain_id, "nonce": n // accounts, "to": "0x" + "77" * 20, "value": 1,
                "gas": 21_000, "maxFeePerGas": base_fee * 4 + 10**9, "maxPriorityFeePerGas": 10**9,
                "type": 2,
            }
            signed.append(k.sign_transaction(tx).raw_transaction.hex())
        start = time.time()
        for i in range(0, transfers, 500):
            rpc("", [], [{"jsonrpc": "2.0", "id": j, "method": "eth_sendRawTransaction", "params": ["0x" + s]}
                         for j, s in enumerate(signed[i:i + 500])])
        first_block = int(rpc("eth_blockNumber", [])["result"], 16)
        mined = 0
        blocks = 0
        while mined < transfers and time.time() - start < 300:
            head = int(rpc("eth_blockNumber", [])["result"], 16)
            mined = sum(
                len(rpc("eth_getBlockByNumber", [hex(b), False])["result"]["transactions"])
                for b in range(first_block, head + 1)
            )
            blocks = head - first_block + 1
            if mined < transfers:
                time.sleep(0.2)
        secs = time.time() - start
        print(
            f"anvil ({version}), block time 1 s: {mined}/{transfers} ETH transfers from {accounts} accounts "
            f"mined in {secs:.2f} s = {mined / secs:.0f} tx/s over {blocks} blocks "
            "(single-process dev node: no consensus, no peers)"
        )
    finally:
        anvil.terminate()


if __name__ == "__main__":
    main()
