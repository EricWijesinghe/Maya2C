"""Bech32m text form of an account address (BIP-350, ADR-081).

Same layout as ``crates/address``: ``<hrp>1`` + version symbol + 52 payload
symbols + 6 checksum symbols. Standard library only. The shared vectors are in
``spec/address-vectors.json``; the Rust crate and the TypeScript SDK read them too.
"""

from __future__ import annotations

from typing import Final, Literal

Network = Literal["mainnet", "testnet", "devnet"]

_NETWORKS: Final[tuple[Network, ...]] = ("mainnet", "testnet", "devnet")
_HRP: Final[dict[Network, str]] = {"mainnet": "maya", "testnet": "tmaya", "devnet": "dmaya"}
_CHARSET: Final = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
_GENERATOR: Final = (0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3)
_BECH32M_CONST: Final = 0x2BC830A3
_CHECKSUM_LEN: Final = 6
_MAX_LEN: Final = 90
_VERSION: Final = 0
_PAYLOAD_SYMBOLS: Final = 52
_DATA_SYMBOLS: Final = 1 + _PAYLOAD_SYMBOLS
ADDRESS_LEN: Final = 32


class AddressError(ValueError):
    """A refused address. ``kind`` is the same name the Rust crate's error uses."""

    def __init__(self, kind: str, message: str) -> None:
        super().__init__(message)
        self.kind = kind


def _polymod(values: list[int]) -> int:
    chk = 1
    for v in values:
        top = chk >> 25
        chk = ((chk & 0x1FFFFFF) << 5) ^ v
        for i, generator in enumerate(_GENERATOR):
            if (top >> i) & 1:
                chk ^= generator
    return chk


def _expand_hrp(hrp: str) -> list[int]:
    return [ord(c) >> 5 for c in hrp] + [0] + [ord(c) & 31 for c in hrp]


def _to_symbols(data: bytes) -> list[int]:
    out: list[int] = []
    acc = 0
    bits = 0
    for b in data:
        acc = (acc << 8) | b
        bits += 8
        while bits >= 5:
            bits -= 5
            out.append((acc >> bits) & 31)
        acc &= (1 << bits) - 1
    if bits > 0:
        out.append((acc << (5 - bits)) & 31)
    return out


def _from_symbols(symbols: list[int]) -> bytes:
    out = bytearray()
    acc = 0
    bits = 0
    for s in symbols:
        acc = (acc << 5) | s
        bits += 5
        while bits >= 8:
            bits -= 8
            out.append((acc >> bits) & 0xFF)
        acc &= (1 << bits) - 1
    # Non-zero leftover padding means a non-canonical string: refuse it.
    if bits >= 5 or acc != 0 or len(out) != ADDRESS_LEN:
        raise AddressError("Payload", "address payload is malformed")
    return bytes(out)


def _create_checksum(hrp: str, data: list[int]) -> list[int]:
    mod = _polymod(_expand_hrp(hrp) + data + [0] * _CHECKSUM_LEN) ^ _BECH32M_CONST
    return [(mod >> (5 * (_CHECKSUM_LEN - 1 - i))) & 31 for i in range(_CHECKSUM_LEN)]


def _split(text: str) -> tuple[str, list[int]]:
    if len(text) > _MAX_LEN:
        raise AddressError("Length", f"address is {len(text)} characters; the limit is 90")
    has_lower = any("a" <= c <= "z" for c in text)
    has_upper = any("A" <= c <= "Z" for c in text)
    if has_lower and has_upper:
        raise AddressError("MixedCase", "address mixes upper and lower case")
    lower = text.lower()
    sep = lower.rfind("1")
    if sep < 0:
        raise AddressError("NoSeparator", "address has no '1' separator")
    hrp = lower[:sep]
    if not hrp:
        raise AddressError("EmptyHrp", "address has no network prefix before the '1'")
    for c in hrp:
        if not 33 <= ord(c) <= 126:
            raise AddressError("InvalidCharacter", f"address contains {c!r}")
    body = lower[sep + 1 :]
    if len(body) != _DATA_SYMBOLS + _CHECKSUM_LEN:
        raise AddressError("Payload", "address payload is malformed")
    data: list[int] = []
    for c in body:
        at = _CHARSET.find(c)
        if at < 0:
            raise AddressError("InvalidCharacter", f"address contains {c!r}")
        data.append(at)
    if _polymod(_expand_hrp(hrp) + data) != _BECH32M_CONST:
        raise AddressError("BadChecksum", "address checksum does not match; a character is mistyped")
    return hrp, data[:_DATA_SYMBOLS]


def _parse(text: str) -> tuple[str, bytes]:
    hrp, data = _split(text)
    if data[0] != _VERSION:
        raise AddressError("Version", f"address format version {data[0]} is not known")
    return hrp, _from_symbols(data[1:])


def encode_address(network: Network, account_id: bytes) -> str:
    """Render a 32-byte account id as its Bech32m text form on ``network``."""
    if len(account_id) != ADDRESS_LEN:
        raise ValueError(f"an account id is {ADDRESS_LEN} bytes, got {len(account_id)}")
    hrp = _HRP[network]
    data = [_VERSION] + _to_symbols(account_id)
    checksum = _create_checksum(hrp, data)
    return hrp + "1" + "".join(_CHARSET[s] for s in data + checksum)


def decode_address(expected: Network, text: str) -> bytes:
    """Decode an address that must be for ``expected``.

    Raises ``AddressError`` with ``kind == "WrongNetwork"`` for a valid address on another network.
    """
    hrp, account_id = _parse(text)
    if hrp != _HRP[expected]:
        raise AddressError(
            "WrongNetwork",
            f"address is for network prefix '{hrp}', but this network expects '{_HRP[expected]}'",
        )
    return account_id


def decode_any_address(text: str) -> tuple[Network, bytes]:
    """Decode an address for whichever known network its prefix names."""
    hrp, account_id = _parse(text)
    for network in _NETWORKS:
        if _HRP[network] == hrp:
            return network, account_id
    raise AddressError("UnknownNetwork", f"address prefix '{hrp}' names no known network")
