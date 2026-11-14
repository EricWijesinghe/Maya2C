"""The v4 payload codec against spec/tests/v4-client-vectors.json (ADR-135 I7).

The vectors are written by the node (crates/node/tests/v4_client_vectors.rs),
so a byte or a word this SDK gets differently fails here.
"""

import json
import pathlib
import unittest

from maya2c_sdk.v4 import PayloadError, decode, describe, encode

VECTORS = pathlib.Path(__file__).resolve().parents[3] / "spec" / "tests" / "v4-client-vectors.json"


class V4Vectors(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.vectors = json.loads(VECTORS.read_text(encoding="utf-8"))["vectors"]

    def test_every_form_is_covered(self) -> None:
        labels = {v["label"] for v in self.vectors}
        self.assertEqual(len(labels), 14)

    def test_encode_matches_the_node_bytes(self) -> None:
        for v in self.vectors:
            with self.subTest(form=v["label"], fields=v["fields"]):
                self.assertEqual(encode(v["label"], v["fields"]).hex(), v["payload"])

    def test_decode_round_trips_to_the_same_fields(self) -> None:
        for v in self.vectors:
            with self.subTest(form=v["label"]):
                self.assertEqual(decode(bytes.fromhex(v["payload"])), (v["label"], v["fields"]))

    def test_describe_uses_the_node_words(self) -> None:
        for v in self.vectors:
            with self.subTest(form=v["label"]):
                summary, careful, details = describe(v["label"], v["fields"])
                self.assertEqual(summary, v["summary"])
                self.assertEqual(careful, v["careful"])
                self.assertEqual([list(d) for d in details], v["details"])

    def test_truncated_trailing_and_unknown_payloads_are_refused(self) -> None:
        for v in self.vectors:
            raw = bytes.fromhex(v["payload"])
            with self.subTest(form=v["label"]):
                with self.assertRaises(PayloadError):
                    decode(raw[:-1])
                with self.assertRaises(PayloadError):
                    decode(raw + b"\0")
        for tag in (0, 54, 69, 255):
            with self.assertRaises(PayloadError):
                decode(bytes([tag]) + bytes(64))

    def test_bad_fields_are_refused_not_truncated(self) -> None:
        with self.assertRaises(PayloadError):
            encode("renew_name", {"label": "Alice", "periods": "1"})
        with self.assertRaises(PayloadError):
            encode("renew_name", {"label": "alice", "periods": "256"})
        with self.assertRaises(PayloadError):
            encode("burn_asset", {"asset": "00" * 31, "amount": "1"})
        with self.assertRaises(PayloadError):
            encode("burn_asset", {"asset": "00" * 32, "amount": str(1 << 256)})
        with self.assertRaises(PayloadError):
            encode("update_name", {"label": "alice", "op": "set_stealth"})


if __name__ == "__main__":
    unittest.main()
