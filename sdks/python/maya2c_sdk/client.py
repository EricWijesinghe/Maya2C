"""JSON-RPC 2.0 client for a Maya2C node."""

from __future__ import annotations

import itertools
import json
import time
import urllib.error
import urllib.request
from typing import Any, Callable


class RpcError(Exception):
    """The node answered with a JSON-RPC error, or not at all."""

    def __init__(self, method: str, detail: Any) -> None:
        super().__init__(f"{method}: {detail}")
        self.method = method
        self.detail = detail


class Client:
    """A node's JSON-RPC endpoint, e.g. ``http://127.0.0.1:8545``."""

    def __init__(self, url: str, timeout: float = 10.0) -> None:
        self.url = url
        self.timeout = timeout
        self._ids = itertools.count(1)

    def call(self, method: str, params: list[Any] | None = None) -> Any:
        """Calls ``method`` and returns its result, raising :class:`RpcError`."""
        body = json.dumps({"jsonrpc": "2.0", "id": next(self._ids), "method": method, "params": params or []}).encode()
        request = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                reply = json.load(response)
        except (urllib.error.URLError, TimeoutError, ValueError) as error:
            raise RpcError(method, error) from error
        if "error" in reply:
            raise RpcError(method, reply["error"])
        return reply.get("result")

    def balance(self, address: str) -> int:
        """The account's balance, in base units."""
        return int(self.call("get_balance", [address])["balance"])

    def account_at_tip(self, address: str) -> dict[str, Any]:
        """The account (address, balance, nonce) with the tip's height and block id, read together."""
        return self.call("get_account_at_tip", [address])

    def block(self, height: int) -> dict[str, Any]:
        """The block at ``height`` on the node's best chain."""
        return self.call("get_block_by_height", [height])

    def fee_info(self) -> dict[str, Any]:
        """The fee market's current state."""
        return self.call("get_fee_info", [])

    def send_raw_transaction(self, raw_hex: str) -> str:
        """Submits a signed transaction; returns its txid."""
        result = self.call("send_raw_transaction", [raw_hex])
        return result["txid"] if isinstance(result, dict) else str(result)

    def wait_until(self, check: Callable[[], bool], timeout: float = 60.0, poll: float = 0.5) -> float:
        """Polls ``check`` until it holds; returns the seconds waited."""
        started = time.monotonic()
        while time.monotonic() - started < timeout:
            if check():
                return time.monotonic() - started
            time.sleep(poll)
        raise TimeoutError(f"not within {timeout} s")
