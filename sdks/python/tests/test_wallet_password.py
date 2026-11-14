"""The wallet password reaches l1-wallet in its environment, never in argv (Standing Order 5)."""

import unittest
from unittest import mock

from maya2c_sdk.client import Client
from maya2c_sdk.wallet import Wallet

PASSWORD = "hunter2-never-on-argv"


class WalletPasswordTest(unittest.TestCase):
    def test_password_is_in_env_and_not_in_argv(self) -> None:
        wallet = Wallet("/opt/l1-wallet", "/keys/main.json", PASSWORD, Client("http://127.0.0.1:8545"))
        done = mock.Mock(returncode=0, stdout="", stderr="")
        with mock.patch("maya2c_sdk.wallet.subprocess.run", return_value=done) as run:
            wallet._output("send", "--to", "maya1x", "--amount", "5", "--no-broadcast")

        command = run.call_args.args[0]
        env = run.call_args.kwargs["env"]
        self.assertFalse(any(PASSWORD in arg for arg in command), command)
        self.assertEqual(env["L1_WALLET_PASSWORD"], PASSWORD)


if __name__ == "__main__":
    unittest.main()
