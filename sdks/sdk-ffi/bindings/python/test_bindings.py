"""Tests for the generated Python bindings.

These load the real cdylib and call through the real FFI boundary. That is the
point: `sdks/sdk-ffi/src/lib.rs` has its own Rust tests, and those prove the
primitives work. What they cannot prove is that the *bindings* work — that the
generated marshalling round-trips a 11,165-byte signature, that an opaque
handle survives, that an error variant arrives as a Python exception.

Run with:

    python -m pytest sdks/sdk-ffi/bindings/python/test_bindings.py

or, without pytest:

    python sdks/sdk-ffi/bindings/python/test_bindings.py
"""

import os
import sys
import unittest

# The generated module loads the cdylib from its own directory, so the release
# build is copied next to it by the test runner. Adding the directory to the
# path is what lets this file be run from the repository root.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import maya_sdk_ffi  # noqa: E402


SEED = bytes([7] * 32)
MESSAGE = b"maya2c"


class TestSigningKey(unittest.TestCase):
    def test_a_seed_of_the_wrong_length_is_refused(self):
        with self.assertRaises(Exception):
            maya_sdk_ffi.SigningKey.from_seed(bytes(31))

    def test_the_same_seed_derives_the_same_key(self):
        first = maya_sdk_ffi.SigningKey.from_seed(SEED)
        second = maya_sdk_ffi.SigningKey.from_seed(SEED)
        self.assertEqual(first.address(), second.address())
        self.assertEqual(first.public_key(), second.public_key())

    def test_an_address_is_64_hex_characters(self):
        key = maya_sdk_ffi.SigningKey.from_seed(SEED)
        address = key.address()
        self.assertEqual(len(address), 64)
        int(address, 16)  # raises if not hex

    def test_a_signature_round_trips_across_the_boundary(self):
        # The check the Rust tests cannot make: an 11,165-byte return value
        # marshalled into a Python `bytes` and back into Rust intact.
        key = maya_sdk_ffi.SigningKey.from_seed(SEED)
        signature = key.sign(MESSAGE)
        self.assertEqual(len(signature), 11165)
        maya_sdk_ffi.verify(key.public_key(), MESSAGE, signature)

    def test_a_tampered_message_does_not_verify(self):
        key = maya_sdk_ffi.SigningKey.from_seed(SEED)
        signature = key.sign(MESSAGE)
        with self.assertRaises(Exception):
            maya_sdk_ffi.verify(key.public_key(), b"tampered", signature)

    def test_a_mis_sized_signature_is_refused(self):
        key = maya_sdk_ffi.SigningKey.from_seed(SEED)
        with self.assertRaises(Exception):
            maya_sdk_ffi.verify(key.public_key(), MESSAGE, bytes(10))

    def test_generated_keys_differ(self):
        self.assertNotEqual(
            maya_sdk_ffi.SigningKey.generate().address(),
            maya_sdk_ffi.SigningKey.generate().address(),
        )

    def test_the_address_agrees_with_the_standalone_function(self):
        key = maya_sdk_ffi.SigningKey.from_seed(SEED)
        self.assertEqual(
            key.address(),
            maya_sdk_ffi.address_from_public_key(key.public_key()),
        )

    def test_there_is_no_way_to_export_the_secret(self):
        # The rule the whole crate is shaped around, asserted where a Python
        # caller would actually try it. If a `secret_bytes` or similar is ever
        # added, this fails and the reviewer is told why it matters.
        # Asserted as an exact surface rather than a keyword filter. A filter
        # flags `from_seed`, which is a *constructor* — secret material
        # entering, which is allowed and documented — and misses an exporter
        # that happened to be called `export` or `raw`. Pinning the whole
        # public surface fails on anything added, whatever it is named.
        key = maya_sdk_ffi.SigningKey.from_seed(SEED)
        surface = sorted(name for name in dir(key) if not name.startswith("_"))
        self.assertEqual(
            surface,
            ["address", "from_seed", "generate", "public_key", "sign"],
            "the signing key's public surface changed; if something was added, "
            "confirm it cannot return secret material before updating this list",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
