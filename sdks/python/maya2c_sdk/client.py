"""JSON-RPC 2.0 client for a Maya2C node."""

from __future__ import annotations

import http.client
import itertools
import json
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Callable

# A reply is read into memory whole; a block at the 5 MiB cap is about 10 MiB as
# hex, so 32 MiB leaves room while a hostile endpoint cannot exhaust memory.
MAX_REPLY_BYTES = 32 * 1024 * 1024
_READ_CHUNK = 64 * 1024


def _read_reply(response: http.client.HTTPResponse, deadline: float) -> bytes:
    """The whole body, refused past MAX_REPLY_BYTES, past ``deadline``, or short of its Content-Length."""
    chunks: list[bytes] = []
    size = 0
    # read1, not read: read(n) blocks until n bytes arrive, so a drip would outlast the deadline.
    while chunk := response.read1(_READ_CHUNK):
        size += len(chunk)
        if size > MAX_REPLY_BYTES:
            raise ValueError(f"reply larger than {MAX_REPLY_BYTES} bytes")
        if time.monotonic() > deadline:
            raise TimeoutError("reply not read within the timeout")
        chunks.append(chunk)
    if response.length:  # read1 ends quietly at EOF, even short of Content-Length
        raise http.client.IncompleteRead(b"".join(chunks), response.length)
    return b"".join(chunks)


class RpcError(Exception):
    """The node answered with a JSON-RPC error, or not at all."""

    def __init__(self, method: str, detail: Any) -> None:
        super().__init__(f"{method}: {detail}")
        self.method = method
        self.detail = detail


class Client:
    """A node's JSON-RPC endpoint, e.g. ``http://127.0.0.1:8545``."""

    def __init__(self, url: str, timeout: float = 10.0) -> None:
        # urlopen also reads file:// and ftp:// URLs; only a node's http(s) endpoint is meant.
        if urllib.parse.urlsplit(url).scheme not in ("http", "https"):
            raise ValueError(f"{url!r} is not an http(s) URL")
        self.url = url
        self.timeout = timeout
        self._ids = itertools.count(1)

    def call(self, method: str, params: list[Any] | None = None) -> Any:
        """Calls ``method`` and returns its result, raising :class:`RpcError`."""
        body = json.dumps({"jsonrpc": "2.0", "id": next(self._ids), "method": method, "params": params or []}).encode()
        request = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
        # urlopen's timeout bounds each recv, not the whole reply.
        deadline = time.monotonic() + self.timeout
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                raw = _read_reply(response, deadline)
            reply = json.loads(raw)
        # OSError covers TimeoutError and a reset mid-body; HTTPException a short body.
        except (urllib.error.URLError, OSError, http.client.HTTPException, ValueError) as error:
            raise RpcError(method, error) from error
        if not isinstance(reply, dict):
            raise RpcError(method, f"reply is not a JSON-RPC object: {type(reply).__name__}")
        if "error" in reply:
            raise RpcError(method, reply["error"])
        return reply.get("result")

    def balance(self, address: str) -> int:
        """The account's balance, in base units."""
        return int(self.call("get_balance", [address])["balance"])

    def account_at_tip(self, address: str) -> dict[str, Any]:
        """The account (address, balance, nonce) with the tip's height and block id, read together."""
        account = self.call("get_account_at_tip", [address])
        # u256 balances arrive as decimal strings (ADR-046); hand callers an int like balance().
        return {**account, "balance": int(account["balance"])}

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
