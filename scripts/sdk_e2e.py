#!/usr/bin/env python3
"""SDK end-to-end: Rust-signed transfer, submitted by the TypeScript SDK.

Starts a one-validator DAG-BFT devnet (reusing `bft_devnet.py`'s setup),
starts `maya2c-gateway` in front of it, signs a transfer with
`l1-wallet send --no-broadcast`, and runs `sdks/sdk-js/test/live-node.test.ts`
against the gateway. Local processes only; nothing leaves 127.0.0.1.

    python scripts/sdk_e2e.py [--workdir D:/Temp/maya-sdk-e2e]
"""

import argparse
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bft_devnet as dv  # noqa: E402

GATEWAY = "127.0.0.1:32080"
AMOUNT = 12_345
BOOT_TIMEOUT_S = 60


def wait_for(url: str) -> None:
    deadline = time.time() + BOOT_TIMEOUT_S
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2):
                return
        except Exception:  # noqa: BLE001 - not up yet
            time.sleep(0.5)
    raise RuntimeError(f"{url} did not come up in {BOOT_TIMEOUT_S}s")


def sign(bins: Path, work: Path) -> dict:
    env = {**os.environ, "L1_WALLET_PASSWORD": dv.PASSWORD}
    out = dv.run([str(bins / f"l1-wallet{dv.EXE}"), "--keystore", str(work / "wallet.key"),
                  "--rpc-url", f"http://127.0.0.1:{dv.RPC_BASE}", "send", "--to", dv.RECIPIENT,
                  "--amount", str(AMOUNT), "--no-broadcast"], env=env)
    fields = dict(line.split(":", 1) for line in out.splitlines() if ":" in line)
    return {k.strip(): v.strip() for k, v in fields.items()}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--workdir", default="D:/Temp/maya-sdk-e2e")
    args = ap.parse_args()
    dv.VALIDATORS = 1
    subprocess.run(["cargo", "build", "-p", "maya2c-node", "-p", "l1-wallet", "-p", "maya2c-peerid",
                    "-p", "maya-api-gateway"], cwd=dv.ROOT, check=True, stdout=subprocess.DEVNULL)
    bins = dv.ROOT / "target" / "debug"
    work = Path(args.workdir)
    _, sender, peers = dv.setup(bins, work)
    procs = dv.start(bins, work, peers)
    gateway = subprocess.Popen(
        [str(bins / f"maya2c-gateway{dv.EXE}"), "--node", f"http://127.0.0.1:{dv.RPC_BASE}",
         "--listen", GATEWAY],
        stdout=open(work / "gateway.log", "w"), stderr=subprocess.STDOUT)
    procs.append(gateway)
    try:
        wait_for(f"http://{GATEWAY}/health")
        # Height 1 first: the fee the wallet computes reads the live base fee.
        deadline = time.time() + BOOT_TIMEOUT_S
        while dv.height(dv.RPC_BASE, 0) < 1 and time.time() < deadline:
            time.sleep(0.5)
        signed = sign(bins, work)
        print(f"signed in Rust: txid {signed['txid']}", flush=True)
        env = {**os.environ, "MAYA_GATEWAY_URL": f"http://{GATEWAY}", "MAYA_RAW_TX": signed["raw"],
               "MAYA_SENDER": sender, "MAYA_RECIPIENT": dv.RECIPIENT, "MAYA_AMOUNT": str(AMOUNT)}
        npx = "npx.cmd" if os.name == "nt" else "npx"
        result = subprocess.run([npx, "vitest", "run", "test/live-node.test.ts"],
                                cwd=dv.ROOT / "sdks" / "sdk-js", env=env)
        return result.returncode
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()


if __name__ == "__main__":
    sys.exit(main())
