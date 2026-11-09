"""Transfers signed by ``l1-wallet``, the Rust wallet, and submitted by :class:`Client`."""

from __future__ import annotations

import os
import subprocess

from .client import Client


class WalletError(Exception):
    """``l1-wallet`` refused, or printed something unexpected."""


class Wallet:
    """A keystore, the ``l1-wallet`` binary that can open it, and a node.

    The password is passed in ``L1_WALLET_PASSWORD`` to the child process
    only; it never appears on a command line.

    ``genesis`` is the network's published genesis id. ``l1-wallet`` signs
    without it only for a network its build pins (mainnet, the public
    testnet); for any other it must be given, and must match the node.
    Calling :meth:`sign_transfer` is the confirmation, so it passes ``--yes``.
    """

    def __init__(
        self, l1_wallet: str, keystore: str, password: str, client: Client, genesis: str | None = None
    ) -> None:
        self.l1_wallet = l1_wallet
        self.genesis = genesis
        self.keystore = keystore
        self._password = password
        self.client = client

    def _output(self, *args: str) -> str:
        env = dict(os.environ, L1_WALLET_PASSWORD=self._password)
        command = [self.l1_wallet, "--keystore", self.keystore, "--rpc-url", self.client.url, *args]
        done = subprocess.run(command, env=env, capture_output=True, text=True, check=False)
        if done.returncode != 0:
            raise WalletError(done.stderr.strip() or f"l1-wallet exited {done.returncode}")
        return done.stdout

    def _run(self, *args: str) -> dict[str, str]:
        """``l1-wallet``'s ``key: value`` lines as a dict."""
        fields = {}
        for line in self._output(*args).splitlines():
            key, sep, value = line.partition(":")
            if sep:
                fields[key.strip()] = value.strip()
        return fields

    def address(self) -> str:
        """The keystore's address as 64 hex characters.

        ``l1-wallet address`` prints the Bech32m text, then ``hex: <64 hex>``
        (ADR-081); a build from before that printed the hex alone.
        """
        printed = self._output("address").strip()
        lines = printed.splitlines()
        hexed = next((v.strip() for k, _, v in (l.partition(":") for l in lines) if k.strip() == "hex"), printed)
        if len(hexed) != 64 or any(c not in "0123456789abcdef" for c in hexed):
            raise WalletError(f"l1-wallet address printed {printed!r}")
        return hexed

    def sign_transfer(self, to: str, amount: int) -> tuple[str, str]:
        """Builds and signs a transfer without broadcasting: ``(raw_hex, txid)``."""
        pin = ("--genesis", self.genesis) if self.genesis else ()
        fields = self._run("send", "--yes", *pin, "--to", to, "--amount", str(amount), "--no-broadcast")
        if "raw" not in fields:
            raise WalletError("l1-wallet printed no raw transaction")
        return fields["raw"], fields.get("txid", "")

    def transfer(self, to: str, amount: int) -> str:
        """Signs a transfer and submits it through the node; returns the txid."""
        raw, _ = self.sign_transfer(to, amount)
        return self.client.send_raw_transaction(raw)
