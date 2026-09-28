"""Against a live devnet (``cargo xtask sdk-e2e --lang python`` sets it up)."""

import os
import unittest

from maya2c_sdk import Client, RpcError, Wallet

ENV = ("MAYA_RPC_URL", "L1_WALLET", "MAYA_KEYSTORE", "L1_WALLET_PASSWORD", "MAYA_RECIPIENT")


@unittest.skipUnless(all(k in os.environ for k in ENV), "needs a devnet: cargo xtask sdk-e2e --lang python")
class LiveNode(unittest.TestCase):
    def setUp(self) -> None:
        self.client = Client(os.environ["MAYA_RPC_URL"])
        self.wallet = Wallet(os.environ["L1_WALLET"], os.environ["MAYA_KEYSTORE"], os.environ["L1_WALLET_PASSWORD"], self.client)

    def test_reads_the_chain(self) -> None:
        sender = self.wallet.address()
        tip = self.client.account_at_tip(sender)
        self.assertGreater(tip["balance"], 0)
        self.assertEqual(len(tip["block_id"]), 64)
        block = self.client.block(1)
        self.assertEqual(block["height"], 1)
        self.assertIn("base_fee", self.client.fee_info())

    def test_a_transfer_signed_in_rust_is_credited(self) -> None:
        recipient = os.environ["MAYA_RECIPIENT"]
        before = self.client.balance(recipient)
        txid = self.wallet.transfer(recipient, 1_234)
        self.assertEqual(len(txid), 64)
        waited = self.client.wait_until(lambda: self.client.balance(recipient) == before + 1_234)
        print(f"python sdk: transfer {txid[:16]}... credited in {waited:.2f} s")

    def test_a_refusal_is_an_error_not_a_silent_success(self) -> None:
        with self.assertRaises(RpcError):
            self.client.send_raw_transaction("00")


if __name__ == "__main__":
    unittest.main()
