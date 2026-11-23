"""The client refuses what a hostile or broken endpoint sends, as RpcError, never a crash or a hang."""

import http.server
import socket
import struct
import threading
import time
import unittest
from typing import Callable

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

    def raw(self, script: Callable[[socket.socket], None], timeout: float = 10.0) -> Client:
        """A server that answers one connection by running ``script`` on its socket."""
        listener = socket.create_server(("127.0.0.1", 0))
        self.addCleanup(listener.close)

        def answer() -> None:
            conn, _ = listener.accept()
            with conn:
                conn.recv(65536)
                try:
                    script(conn)
                except OSError:
                    pass  # the client gave up first

        threading.Thread(target=answer, daemon=True).start()
        return Client(f"http://127.0.0.1:{listener.getsockname()[1]}", timeout=timeout)

    def test_a_body_shorter_than_its_content_length_is_an_rpc_error(self) -> None:
        # Raised inside response.read(), after urlopen stopped wrapping errors.
        def short(conn: socket.socket) -> None:
            conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n" + b'{"result":7}')

        with self.assertRaises(RpcError):
            self.raw(short).call("x")

    def test_a_connection_reset_mid_body_is_an_rpc_error(self) -> None:
        def reset(conn: socket.socket) -> None:
            conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n" + b'{"res')
            conn.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))

        with self.assertRaises(RpcError):
            self.raw(reset).call("x")

    def test_a_slow_drip_is_cut_off_at_the_timeout(self) -> None:
        # Each byte arrives well inside the per-recv timeout; only a whole-reply
        # deadline stops it.
        def drip(conn: socket.socket) -> None:
            conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n")
            for _ in range(1000):
                conn.sendall(b" ")
                time.sleep(0.1)

        started = time.monotonic()
        with self.assertRaises(RpcError):
            self.raw(drip, timeout=0.5).call("x")
        self.assertLess(time.monotonic() - started, 5.0)

    def test_a_well_formed_reply_still_returns_its_result(self) -> None:
        self.assertEqual(self.reply(b'{"jsonrpc":"2.0","id":1,"result":7}').call("x"), 7)


if __name__ == "__main__":
    unittest.main()
