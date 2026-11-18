"""The v4 payload codec against spec/tests/v4-client-vectors.json (ADR-135 I7).

The vectors are written by the node (crates/node/tests/v4_client_vectors.rs),
so a byte or a word this SDK gets differently fails here.
"""

import json
import pathlib
import unittest

from maya2c_sdk.v4 import PayloadError, decode, describe, describe_bytes, encode

VECTORS = pathlib.Path(__file__).resolve().parents[3] / "spec" / "tests" / "v4-client-vectors.json"


class V4Vectors(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.file = json.loads(VECTORS.read_text(encoding="utf-8"))
        cls.vectors = cls.file["vectors"]

    def test_describe_bytes_reads_what_will_be_signed(self) -> None:
        for v in self.vectors:
            with self.subTest(form=v["label"]):
                summary, careful, details = describe_bytes(bytes.fromhex(v["payload"]))
                self.assertEqual((summary, careful), (v["summary"], v["careful"]))
                self.assertEqual([list(d) for d in details], v["details"])

    def test_a_symbol_no_encoder_writes_reads_as_the_node_reads_it(self) -> None:
        self.assertTrue(self.file["decode_only"])
        for v in self.file["decode_only"]:
            with self.subTest(payload=v["payload"]):
                raw = bytes.fromhex(v["payload"])
                self.assertEqual(decode(raw), (v["label"], v["fields"]))
                self.assertEqual([list(d) for d in describe_bytes(raw)[2]], v["details"])
                with self.assertRaises(PayloadError):
                    encode(v["label"], v["fields"])

    def test_what_the_node_refuses_is_refused(self) -> None:
        for r in self.file["refused_payloads"]:
            with self.subTest(why=r["why"]), self.assertRaises(PayloadError):
                decode(bytes.fromhex(r["payload"]))

    def test_fields_every_encoder_refuses_raise_payload_error(self) -> None:
        self.assertTrue(self.file["refused_fields"])
        for r in self.file["refused_fields"]:
            with self.subTest(why=r["why"], fields=r["fields"]), self.assertRaises(PayloadError):
                encode(r["label"], r["fields"])

    def test_an_update_op_it_would_not_encode_is_not_described(self) -> None:
        for op in ("set_stealth", "constructor", ""):
            with self.subTest(op=op), self.assertRaises(PayloadError):
                describe("update_name", {"label": "alice", "op": op, "address": "b2" * 32})

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
