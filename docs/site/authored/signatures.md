---
title: Post-quantum signatures in your application
description: Sign and verify with Maya2C's hybrid ML-DSA-65 + SLH-DSA signature from Python, and what it does and does not protect.
---

Maya2C's signature is available to applications that never touch the chain:
documents, invoices, software releases, API requests. It is the same
construction every Maya2C transaction carries, through the same Rust code.

## What you get

A **hybrid signature**: ML-DSA-65 (NIST FIPS 204, lattice-based) *and*
SLH-DSA-SHA2-128s (NIST FIPS 205, hash-based) over the same message. A
verifier accepts only if **both** verify. A forger has to break both a lattice
problem and a hash-based scheme, and SLH-DSA's security rests only on the hash
function.

| | Size |
|---|---|
| Public key | 1,984 bytes |
| Signature | 11,165 bytes |
| Address (BLAKE3 of the public key) | 32 bytes, 64 hex characters |

**This is signing, not encryption.** It proves who produced a message and that
it was not altered; it does not hide the message. Post-quantum *encryption*
(ML-KEM-768) is used inside Maya2C's network transport, but it is not exposed
as an application library yet.

## Status — read before depending on it

- **Evaluation and testnet use.** The construction and its bindings are tested
  in this repository, but **no independent security audit** of Maya2C's
  cryptography has taken place.
- It builds on two open-source implementations: `fips204` 0.4 for ML-DSA-65,
  and RustCrypto's `slh-dsa` pinned at `0.2.0-rc.5`, a release candidate.
- **Not published to PyPI yet.** `pip install maya2c` does not work today; build
  it from source as below.

For production systems that need certified cryptography, use a FIPS 140-3
validated module. A hybrid like this one is a sound design, but "sound" and
"validated" are different claims.

## Python

The bindings are generated with UniFFI from `sdks/sdk-ffi` and load a compiled
Rust library.

### Build

```bash
git clone https://github.com/EricWijesinghe/Maya2C
cd Maya2C
cargo build -p maya-sdk-ffi --release
```

Copy the library and the generated module into your project, side by side:

| OS | Library in `target/release/` |
|---|---|
| Linux | `libmaya_sdk_ffi.so` |
| macOS | `libmaya_sdk_ffi.dylib` |
| Windows | `maya_sdk_ffi.dll` |

…plus `sdks/sdk-ffi/bindings/python/maya_sdk_ffi.py`.

### Sign and verify

```python
from maya_sdk_ffi import SigningKey, verify, address_from_public_key

key = SigningKey.generate()
print("address:   ", key.address())
print("public key:", len(key.public_key()), "bytes")

message = b"invoice 2026-0042: pay 1,200.00 EUR"
signature = key.sign(message)
print("signature: ", len(signature), "bytes")

verify(key.public_key(), message, signature)          # raises on failure
print("verified:   yes")

try:
    verify(key.public_key(), b"invoice 2026-0042: pay 9,200.00 EUR", signature)
except Exception as error:
    print("tampered:   rejected ->", type(error).__name__)
```

Output from a real run (2026-09-29):

```text
address:    efd70367be6299b044f01da60263382a146abf988d8363659e0f045aa020a316
public key: 1984 bytes
signature:  11165 bytes
verified:   yes
tampered:   rejected -> VerificationFailed
```

On the same run, signing averaged **103 ms** and verifying **0.4 ms** (mean of
20; release build; Windows 11, Intel Core Ultra 9 275HX; the Python call
included). Signing is slow because of the hash-based half. Budget for it
where you sign in bulk; verifying is cheap.

### Keys

- `SigningKey.generate()` draws a fresh key from the OS random source.
- `SigningKey.from_seed(seed)` derives one deterministically from exactly
  32 bytes, for a key you must be able to recreate. Guard that seed as you would
  the key.
- **The secret key cannot be exported.** `SigningKey` offers `public_key()`,
  `address()` and `sign()`, and deliberately nothing that returns the secret
  half. Once in Python, secret bytes could be copied by the interpreter and
  never wiped; inside the Rust object they are zeroised on drop.
- **Signing is deterministic**: the same key and message always give the same
  signature. That is required on-chain, where a transaction id hashes its
  signature, and harmless elsewhere.

## Other languages

| | Status |
|---|---|
| Rust | The node's `crypto::hybrid` module is the reference implementation, but it lives in a crate that links RocksDB. A standalone signing crate is planned. |
| Browser / JavaScript | `sdks/sdk-wasm` and `sdks/sdk-js` (`maya2c.js`), with parity tests against the node. |
| Kotlin, Swift | Bindings are generated in `sdks/sdk-ffi/bindings/` but **have never been compiled or tested**. Do not ship them. |
| Go | `maya2c.dev/sdk` (Go 1.25+) is a chain client; it signs through the `l1-wallet` binary rather than in Go. |
