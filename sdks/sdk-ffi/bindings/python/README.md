# maya2c

Post-quantum signing primitives for the Maya2C network.

```bash
pip install maya2c
```

## What it does

Signs. That is the whole surface, and it is deliberate.

A Maya2C signature is a **hybrid pair** — ML-DSA-65 (FIPS 204, lattice) and
SLH-DSA-SHA2-128s (FIPS 205, hash-based) — 11,165 bytes together, and **both
must verify**. Two families rather than one because a break in the lattice
assumption should not be a break in the chain.

There is no usable pure-Python implementation of either, and an approximate one
would produce transactions the chain rejects — or, worse, that it accepts for
the wrong reason. So this is a compiled extension rather than a port.

```python
from maya2c import SigningKey, verify, address_from_public_key

key = SigningKey.generate()
print(key.address())

signature = key.sign(b"payload")
verify(key.public_key(), b"payload", signature)   # raises SdkError on failure
```

`SigningKey.from_seed(seed)` derives deterministically from 32 bytes, for a key
that has to be reproducible from a mnemonic.

## Two properties worth knowing

**Signing is deterministic.** The same message signed twice with the same key is
byte-identical. This is load-bearing, not incidental: a transaction id hashes
the signature, so a hedged signature would give one transaction two identities.
It also means re-signing and re-broadcasting is safe.

**The secret key never leaves the object.** `SigningKey` exposes `public_key()`,
`address()` and `sign()` — there is no accessor for the secret half, by design.
A key that can be read out is a key that ends up in a log.

## What this is not

It is not a client. There is no RPC, no transaction builder, and no way to reach
a chain from this package. Building and submitting transactions is the
gateway's business — `maya2c.js` does that for TypeScript, and there is no
Python equivalent yet.

## Wheels only

No source distribution is published. The extension embeds RocksDB's C++ through
`custom-l1-node`, so a source build would compile RocksDB and the arkworks stack
on your machine — many minutes when it works, and a failure without a C++
toolchain. Wheels are built per platform in CI instead.

Linux (manylinux), macOS and Windows, CPython 3.9+.

## Status

Pre-launch. The chain this signs for refuses to run on a value-bearing chain id
while its shielded pool's circuit is unaudited — see
`docs/mainnet-readiness.md` in the repository. Keys generated now are real keys;
the network they are for is not yet.
