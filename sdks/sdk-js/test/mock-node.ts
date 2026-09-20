/**
 * A mock gateway, served over real HTTP on a real port.
 *
 * # Why a real server and not a stubbed `fetch`
 *
 * A stubbed `fetch` tests that the client calls the function it was given. It
 * does not test URL construction, JSON encoding, status handling, header
 * negotiation, or the abort path — which is most of what a client is. Every
 * bug this SDK is likely to have lives in exactly the layer a stub replaces.
 *
 * So this listens on a port and the tests speak HTTP to it. It mirrors
 * `maya-api-gateway`'s shapes, including its refusals: the miner routes are
 * absent, the sealed size ceiling is enforced, and upstream errors are flat.
 */

import { createServer, type Server } from "node:http";
import { AddressInfo } from "node:net";

/** Byte ceiling on a sealed payload, mirroring the Rust gateway. */
export const MAX_SEALED_PAYLOAD_BYTES = 64 * 1024;

/** A running mock, plus its URL and a record of what it was asked. */
export interface MockNode {
  /** Base URL, e.g. `http://127.0.0.1:53187`. */
  url: string;
  /** Paths requested, in order. */
  requests: string[];
  /** Stops the server. */
  close(): Promise<void>;
  /** When set, every route answers 502 as the real gateway does upstream. */
  failUpstream: boolean;
}

/** Starts the mock on an ephemeral port. */
export async function startMockNode(): Promise<MockNode> {
  const requests: string[] = [];
  const state = { failUpstream: false };

  const server: Server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://localhost");
    requests.push(url.pathname);

    const send = (status: number, body: unknown): void => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };

    // Read the body first; every POST route needs it.
    let raw = "";
    req.on("data", (chunk) => {
      raw += chunk;
    });
    req.on("end", () => {
      // `/health` answers before the upstream-failure switch, exactly as the
      // Rust gateway does: a health check that proxied upstream would report
      // the gateway unhealthy whenever the node was.
      if (url.pathname === "/health") {
        send(200, { status: "ok" });
        return;
      }

      if (state.failUpstream) {
        // Flat message, as the real gateway returns — the node's address must
        // not reach the client.
        send(502, { error: "upstream node error" });
        return;
      }

      if (url.pathname.startsWith("/v1/accounts/")) {
        const address = url.pathname.slice("/v1/accounts/".length);
        const bare = address.startsWith("0x") ? address.slice(2) : address;
        if (bare.length !== 64 || !/^[0-9a-fA-F]+$/.test(bare)) {
          send(400, { error: "address must be 64 hex characters" });
          return;
        }
        send(200, { balance: 1234, nonce: 7 });
        return;
      }

      if (url.pathname.startsWith("/v1/blocks/")) {
        const height = Number(url.pathname.slice("/v1/blocks/".length));
        if (!Number.isInteger(height)) {
          send(400, { error: "height must be an integer" });
          return;
        }
        if (height > 100) {
          send(404, { error: "not found" });
          return;
        }
        send(200, { height, hash: "ab".repeat(32) });
        return;
      }

      if (url.pathname === "/v1/supply") {
        send(200, { circulating: 21000, total: 42000 });
        return;
      }

      if (url.pathname === "/v1/transactions") {
        const body = JSON.parse(raw || "{}") as { raw?: string };
        if (!body.raw) {
          send(400, { error: "raw is empty" });
          return;
        }
        send(200, { hash: "cd".repeat(32) });
        return;
      }

      if (url.pathname === "/v1/sealed") {
        const body = JSON.parse(raw || "{}") as { ciphertext?: string };
        const ciphertext = body.ciphertext ?? "";
        if (ciphertext.length > MAX_SEALED_PAYLOAD_BYTES * 2) {
          send(413, { error: "payload too large" });
          return;
        }
        if (ciphertext.length === 0 || ciphertext.length % 2 !== 0) {
          send(400, { error: "sealed payload is malformed" });
          return;
        }
        if (!/^[0-9a-fA-F]+$/.test(ciphertext)) {
          send(400, { error: "sealed payload is not valid hex" });
          return;
        }
        send(200, { id: "ef".repeat(32), bytes: ciphertext.length / 2 });
        return;
      }

      if (url.pathname === "/graphql") {
        const body = JSON.parse(raw || "{}") as { query?: string };
        const query = body.query ?? "";
        // Mirrors the Rust schema's complexity limit. Not a real GraphQL
        // engine — the point is that the client handles a 200-with-errors,
        // which is the shape that trips naive clients.
        if ((query.match(/supply/g) ?? []).length > 200) {
          send(200, { errors: [{ message: "query is too complex" }] });
          return;
        }
        if (query.includes("mutation")) {
          send(200, { errors: [{ message: "no mutations are exposed" }] });
          return;
        }
        if (query.includes("supply")) {
          send(200, { data: { supply: { circulating: 21000, total: 42000 } } });
          return;
        }
        send(200, { errors: [{ message: "unknown field" }] });
        return;
      }

      // Everything else — including anything miner-shaped — is absent.
      send(404, { error: "not found" });
    });
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;

  return {
    url: `http://127.0.0.1:${port}`,
    requests,
    get failUpstream() {
      return state.failUpstream;
    },
    set failUpstream(value: boolean) {
      state.failUpstream = value;
    },
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      ),
  };
}
