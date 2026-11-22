"""The client refuses what a hostile or broken endpoint sends, as RpcError, never a crash or a hang."""

import http.server
import threading
import unittest

from maya2c_sdk import Client, RpcError
from maya2c_sdk.client import MAX_REPLY_BYTES


def serve(body: bytes) -> http.server.HTTPServer:
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self) -> None:  # noqa: N802 (stdlib name)
            self.rfile.read(int(self.headers["Content-Length"]))
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_: object) -> None:
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


class Refusals(unittest.TestCase):
    def reply(self, body: bytes) -> Client:
        server = serve(body)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return Client(f"http://127.0.0.1:{server.server_port}")

    def test_a_non_http_url_is_refused_at_construction(self) -> None:
        # urlopen would read file:// and ftp:// URLs as happily as a node.
        for url in ("file:///etc/passwd", "ftp://example.org/", "127.0.0.1:8545"):
            with self.assertRaises(ValueError, msg=url):
                Client(url)
        Client("https://rpc.example.org")

    def test_a_reply_larger_than_the_cap_is_an_rpc_error(self) -> None:
        client = self.reply(b'{"result":"' + b"a" * MAX_REPLY_BYTES + b'"}')
        with self.assertRaises(RpcError):
            client.call("get_fee_info")

    def test_a_reply_that_is_not_an_object_is_an_rpc_error(self) -> None:
        for body in (b"[1, 2]", b'"x"', b"null"):
            with self.assertRaises(RpcError, msg=body):
                self.reply(body).call("get_fee_info")

    def test_a_well_formed_reply_still_returns_its_result(self) -> None:
        self.assertEqual(self.reply(b'{"jsonrpc":"2.0","id":1,"result":7}').call("x"), 7)


if __name__ == "__main__":
    unittest.main()
