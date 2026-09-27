#!/usr/bin/env python3
"""A four-validator DAG-BFT devnet on localhost, run by the real binary.

Builds `maya2c-node`, `l1-wallet` and `maya2c-peerid`, generates four
ML-DSA-65 validator keys and a funded wallet, writes a genesis with a `bft`
committee, starts four node processes plus one observer, sends transfers over
JSON-RPC, and reports what it measured:

- every node's height over time (blocks per second);
- time from `send_raw_transaction` to the transfer being visible on every node;
- whether every node agrees on the block id at the common height.

Nothing here is a benchmark of the chain's ceiling: five processes on one
machine, dev profile, localhost links. It proves the binary runs DAG-BFT end
to end and prints numbers with the hardware they came from.

    python scripts/bft_devnet.py [--workdir D:/Temp/maya-bft-devnet]
                                 [--seconds 60] [--transfers 5] [--release]
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VALIDATORS = 4
P2P_BASE = 31_000
RPC_BASE = 32_000
PASSWORD = "devnet-only-password"
RECIPIENT = "77" * 32
EXE = ".exe" if os.name == "nt" else ""


def build(release: bool) -> Path:
    profile = ["--release"] if release else []
    cmd = ["cargo", "build", *profile, "-p", "maya2c-node", "-p", "l1-wallet", "-p", "maya2c-peerid"]
    print("building:", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    return ROOT / "target" / ("release" if release else "debug")


def run(cmd: list[str], **kw) -> str:
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw).stdout


def rpc(port: int, method: str, *params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": list(params)}).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}", data=body, headers={"content-type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        reply = json.loads(resp.read())
    if "error" in reply:
        raise RuntimeError(reply["error"])
    return reply["result"]


def height(port: int, hint: int) -> int:
    """Highest height with a block, searched up from `hint`."""
    h = hint
    try:
        while True:
            rpc(port, "get_block_by_height", h + 1)
            h += 1
    except Exception:  # noqa: BLE001 - "no block at height" ends the walk
        return h


def block_id(port: int, h: int) -> str:
    b = rpc(port, "get_block_by_height", h)
    return b.get("id") or b.get("hash") or json.dumps(b, sort_keys=True)


def setup(bins: Path, work: Path) -> tuple[list[str], str, list[str]]:
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    node = str(bins / f"maya2c-node{EXE}")
    pubkeys = []
    for i in range(VALIDATORS):
        d = work / f"v{i}"
        d.mkdir()
        pubkeys.append(run([node, "--generate-validator-key", str(d / "validator.key")]).strip())
    wallet = str(bins / f"l1-wallet{EXE}")
    env = {**os.environ, "L1_WALLET_PASSWORD": PASSWORD}
    out = run([wallet, "--keystore", str(work / "wallet.key"), "generate"], env=env)
    address = next(l.split()[-1] for l in out.splitlines() if l.startswith("address:"))
    genesis = {
        "chain_id": "maya-bft-devnet",
        "timestamp": int(time.time()),
        "difficulty_bits": 0,
        "pow_limit_bits": 0,
        "allocations": [{"address": address, "balance": 10_000_000}],
        "bft": {"validators": pubkeys, "anchor_timeout_ms": 1000, "batch_size": 500},
    }
    (work / "genesis.json").write_text(json.dumps(genesis, indent=2))
    peer = str(bins / f"maya2c-peerid{EXE}")
    peers = []
    for i in range(VALIDATORS + 1):
        d = work / (f"v{i}" if i < VALIDATORS else "observer")
        d.mkdir(exist_ok=True)
        peers.append(run([peer, "--data-dir", str(d), "--create"]).strip().split()[-1])
    return pubkeys, address, peers


def start(bins: Path, work: Path, peers: list[str]) -> list[subprocess.Popen]:
    node = str(bins / f"maya2c-node{EXE}")
    procs = []
    for i in range(VALIDATORS + 1):
        d = work / (f"v{i}" if i < VALIDATORS else "observer")
        cmd = [
            node, "--genesis", str(work / "genesis.json"), "--data-dir", str(d),
            "--rpc-addr", f"127.0.0.1:{RPC_BASE + i}", "--p2p-port", str(P2P_BASE + i),
        ]
        for j in range(VALIDATORS + 1):
            if j != i:
                cmd += ["--bootnode", f"/ip4/127.0.0.1/tcp/{P2P_BASE + j}/p2p/{peers[j]}"]
        if i < VALIDATORS:
            cmd += ["--validator-key", str(d / "validator.key")]
        log = open(d / "node.log", "w")
        procs.append(subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT))
    return procs


def send(bins: Path, work: Path, port: int, amount: int, nonce: int) -> None:
    wallet = str(bins / f"l1-wallet{EXE}")
    env = {**os.environ, "L1_WALLET_PASSWORD": PASSWORD}
    run([wallet, "--keystore", str(work / "wallet.key"), "--rpc-url", f"http://127.0.0.1:{port}",
         "send", "--to", RECIPIENT, "--amount", str(amount), "--nonce", str(nonce)], env=env)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--workdir", default="D:/Temp/maya-bft-devnet")
    ap.add_argument("--seconds", type=int, default=60)
    ap.add_argument("--transfers", type=int, default=5)
    ap.add_argument("--release", action="store_true")
    args = ap.parse_args()
    work = Path(args.workdir)
    bins = build(args.release)
    _, _, peers = setup(bins, work)
    procs = start(bins, work, peers)
    ports = [RPC_BASE + i for i in range(VALIDATORS + 1)]
    report: dict = {
        "hardware": {"machine": platform.machine(), "processor": platform.processor(),
                     "system": f"{platform.system()} {platform.release()}",
                     "cpus": os.cpu_count()},
        "profile": "release" if args.release else "dev",
        "commit": run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT).strip(),
    }
    try:
        deadline = time.time() + 60
        while time.time() < deadline:
            try:
                if all(height(p, 0) >= 0 for p in ports):
                    break
            except Exception:  # noqa: BLE001 - RPC not up yet
                pass
            time.sleep(1)
        t0 = time.time()
        h0 = [height(p, 0) for p in ports]
        latencies = []
        for n in range(args.transfers):
            sent = time.time()
            send(bins, work, ports[n % VALIDATORS], 100, n)
            expected = 100 * (n + 1)
            while time.time() - sent < 60:
                bals = [int(rpc(p, "get_balance", RECIPIENT)["balance"]) for p in ports]
                if all(b >= expected for b in bals):
                    latencies.append(round(time.time() - sent, 3))
                    break
                time.sleep(0.1)
            else:
                latencies.append(None)
        while time.time() - t0 < args.seconds:
            time.sleep(1)
        elapsed = time.time() - t0
        h1 = [height(p, h) for p, h in zip(ports, h0)]
        common = min(h1)
        ids = [block_id(p, common) for p in ports]
        balances = [int(rpc(p, "get_balance", RECIPIENT)["balance"]) for p in ports]
        report |= {
            "elapsed_s": round(elapsed, 1),
            "heights": h1,
            "blocks_per_s": [round((b - a) / elapsed, 2) for a, b in zip(h0, h1)],
            "transfer_visible_on_all_nodes_s": latencies,
            "agree_at_common_height": len(set(ids)) == 1,
            "common_height": common,
            "recipient_balance_per_node": balances,
        }
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()
    print(json.dumps(report, indent=2))
    ok = (report.get("agree_at_common_height") and report.get("common_height", 0) > 0
          and all(l is not None for l in report.get("transfer_visible_on_all_nodes_s", [None]))
          and len(set(report.get("recipient_balance_per_node", [0, 1]))) == 1)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
