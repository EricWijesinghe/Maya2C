/**
 * End-to-end tests: the real client against a real HTTP server.
 *
 * Nothing is stubbed. Each test starts a mock gateway on an ephemeral port and
 * the client speaks HTTP to it, so URL construction, JSON handling, status
 * mapping, and the abort path are all exercised — the layer a stubbed `fetch`
 * would have replaced, and where a client's bugs actually live.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { GatewayError, MayaClient } from "../src/index.js";
import { MAX_SEALED_PAYLOAD_BYTES, startMockNode, type MockNode } from "./mock-node.js";

let node: MockNode;
let client: MayaClient;

const ADDRESS = "ab".repeat(32);

beforeEach(async () => {
  node = await startMockNode();
  client = new MayaClient(node.url);
});

afterEach(async () => {
  await node.close();
});

describe("reads", () => {
  it("reports gateway health without touching the node", async () => {
    await expect(client.health()).resolves.toEqual({ status: "ok" });
  });

  it("fetches a balance", async () => {
    await expect(client.getBalance(ADDRESS)).resolves.toEqual({
      balance: 1234,
      nonce: 7,
    });
  });

  it("accepts an address with or without a 0x prefix", async () => {
    await expect(client.getBalance(`0x${ADDRESS}`)).resolves.toMatchObject({
      nonce: 7,
    });
  });

  it("fetches a block", async () => {
    await expect(client.getBlock(1)).resolves.toMatchObject({ height: 1 });
  });

  it("fetches supply", async () => {
    await expect(client.getSupply()).resolves.toEqual({
      circulating: 21000,
      total: 42000,
    });
  });

  it("strips trailing slashes from the base URL", async () => {
    // A double slash is a different path to some proxies, so this is not
    // cosmetic — it is the difference between a 200 and a 404 in production.
    const trailing = new MayaClient(`${node.url}///`);
    await expect(trailing.health()).resolves.toEqual({ status: "ok" });
    expect(node.requests).toContain("/health");
  });
});

describe("writes", () => {
  it("submits a signed transaction", async () => {
    await expect(client.sendRawTransaction("deadbeef")).resolves.toHaveProperty(
      "hash",
    );
  });

  it("submits a sealed transaction", async () => {
    const accepted = await client.sendSealedTransaction("deadbeef");
    expect(accepted.bytes).toBe(4);
  });

  it("surfaces the gateway's refusal of an oversized sealed payload", async () => {
    const oversized = "ab".repeat(MAX_SEALED_PAYLOAD_BYTES + 1);
    await expect(
      client.sendSealedTransaction(oversized),
    ).rejects.toMatchObject({ status: 413 });
  });

  it("surfaces the gateway's refusal of a non-hex sealed payload", async () => {
    await expect(client.sendSealedTransaction("zzzz")).rejects.toMatchObject({
      status: 400,
    });
  });
});

describe("errors", () => {
  it("marks 4xx as not retryable and 5xx as retryable", async () => {
    // The distinction the status is kept for: a client that collapsed both
    // into a string would retry malformed requests forever.
    const badAddress = client.getBalance("nothex");
    await expect(badAddress).rejects.toSatisfy(
      (e: GatewayError) => e.status === 400 && !e.retryable,
    );

    node.failUpstream = true;
    await expect(client.getSupply()).rejects.toSatisfy(
      (e: GatewayError) => e.status === 502 && e.retryable,
    );
  });

  it("does not leak the node address on an upstream failure", async () => {
    node.failUpstream = true;
    // `.then(onFulfilled, onRejected)` rather than `.catch`: the two-argument
    // form types as `GatewayError` alone, where `.catch` widens to
    // `Supply | GatewayError`. It also makes a *resolved* call fail loudly here
    // instead of asserting `.message` on a supply report and reporting a
    // confusing mismatch.
    const error = await client.getSupply().then(
      () => {
        throw new Error("getSupply should have rejected on an upstream failure");
      },
      (e: GatewayError) => e,
    );
    expect(error.message).toBe("upstream node error");
    expect(error.message).not.toMatch(/\d+\.\d+\.\d+\.\d+/);
  });

  it("returns 404 for a missing block", async () => {
    await expect(client.getBlock(9999)).rejects.toMatchObject({ status: 404 });
  });

  it("rejects a negative or fractional height before making a request", async () => {
    const before = node.requests.length;
    await expect(client.getBlock(-1)).rejects.toBeInstanceOf(GatewayError);
    await expect(client.getBlock(1.5)).rejects.toBeInstanceOf(GatewayError);
    expect(node.requests.length).toBe(before);
  });

  it("times out rather than hanging", async () => {
    // A wedged gateway must not leave a browser spinner running forever.
    const slow = new MayaClient("http://127.0.0.1:9", { timeoutMs: 50 });
    await expect(slow.health()).rejects.toBeInstanceOf(GatewayError);
  });
});

describe("the miner interface is not reachable", () => {
  it("has no route for mining or block submission", async () => {
    // The gateway's allowlist is enforced in Rust; this asserts the
    // consequence from where a client stands, which is where an attacker
    // stands too.
    for (const path of [
      "/v1/mining/candidate",
      "/get_mining_candidate",
      "/submit_block",
    ]) {
      const response = await fetch(`${node.url}${path}`);
      expect(response.status).toBe(404);
    }
  });
});

describe("graphql", () => {
  it("runs a query", async () => {
    const data = await client.graphql<{ supply: { circulating: number } }>(
      "{ supply { circulating total } }",
    );
    expect(data.supply.circulating).toBe(21000);
  });

  it("throws on a 200 that carries errors", async () => {
    // GraphQL returns 200 with an `errors` array. A client that only checked
    // the status would treat a rejected query as a success with null data —
    // the single most common GraphQL client bug.
    await expect(
      client.graphql("mutation { sendRawTransaction(raw: \"ab\") }"),
    ).rejects.toBeInstanceOf(GatewayError);
  });

  it("surfaces a complexity-limit refusal", async () => {
    const wide = `{ ${Array.from({ length: 300 }, (_, i) => `f${i}: supply { circulating }`).join(" ")} }`;
    await expect(client.graphql(wide)).rejects.toThrow(/too complex/);
  });
});
