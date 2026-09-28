"""Maya2C for Python: a JSON-RPC client, and transfers signed by the Rust wallet.

This package never implements the hybrid ML-DSA-65 + SLH-DSA signature or the
transaction wire format. Both are consensus rules, and a second implementation
is a second thing that can disagree with the chain. Signing runs through
``l1-wallet`` (the Rust wallet shipped with the node); this package builds the
request, submits the result and reads the chain. Standard library only.
"""

from .client import Client, RpcError
from .wallet import Wallet, WalletError

__all__ = ["Client", "RpcError", "Wallet", "WalletError"]
