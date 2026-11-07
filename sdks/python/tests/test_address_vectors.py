"""Cross-language check of the address text form against spec/address-vectors.json (ADR-081).

The same file is read by crates/address and sdks/sdk-js, so a change to the
text form fails in all three at once.
"""

import json
import pathlib
import unittest

from maya2c_sdk.address import AddressError, decode_address, decode_any_address, encode_address

VECTORS = pathlib.Path(__file__).resolve().parents[3] / "spec" / "address-vectors.json"


class AddressVectors(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.vectors = json.loads(VECTORS.read_text(encoding="utf-8"))

    def test_has_the_full_vector_set(self) -> None:
        self.assertGreaterEqual(len(self.vectors["valid"]), 13)
        self.assertGreaterEqual(len(self.vectors["refused"]), 8)

    def test_valid_vectors_encode_and_decode_exactly(self) -> None:
        for case in self.vectors["valid"]:
            with self.subTest(case=case["name"]):
                account_id = bytes.fromhex(case["id_hex"])
                network = case["network"]
                self.assertEqual(encode_address(network, account_id), case["text"].lower())
                self.assertEqual(decode_address(network, case["text"]), account_id)
                self.assertEqual(decode_any_address(case["text"]), (network, account_id))

    def test_refused_vectors_fail_with_the_named_error(self) -> None:
        for case in self.vectors["refused"]:
            with self.subTest(case=case["name"]):
                with self.assertRaises(AddressError) as ctx:
                    if case["expect_network"] is None:
                        decode_any_address(case["text"])
                    else:
                        decode_address(case["expect_network"], case["text"])
                self.assertEqual(ctx.exception.kind, case["error"])

    def test_wrong_length_account_id_is_refused(self) -> None:
        with self.assertRaises(ValueError):
            encode_address("testnet", bytes(31))


if __name__ == "__main__":
    unittest.main()
