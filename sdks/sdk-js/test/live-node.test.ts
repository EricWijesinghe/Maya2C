/**
 * The SDK against a real node, through the real gateway.
 *
 * Skipped unless `MAYA_GATEWAY_URL` is set: `cargo xtask sdk-e2e` starts a
 * one-validator DAG-BFT devnet and `maya2c-gateway`, signs a transfer with the
 * Rust wallet (`l1-wallet send --no-broadcast`), and runs this file with:
 *
 * - `MAYA_GATEWAY_URL` — the gateway;
 * - `MAYA_RAW_TX` — the signed transfer, hex;
 * - `MAYA_SENDER`, `MAYA_RECIPIENT` — hex addresses;
 * - `MAYA_AMOUNT` — what the transfer pays the recipient.
 *
 * So the transaction is built in Rust, submitted from TypeScript, and executed
 * and finalized by the node: the cross-language path, with nothing mocked.
 */

import { describe, expect, it } from "vitest";
import { GatewayError, MayaClient } from "../src/index.js";

const env = process.env;
const url = env.MAYA_GATEWAY_URL;
const FINALITY_WAIT_MS = 60_000;
const POLL_MS = 500;

async function until<T>(read: () => Promise<T>, done: (value: T) => boolean): Promise<T> {
  const deadline = Date.now() + FINALITY_WAIT_MS;
  for (;;) {
    const value = await read();
    if (done(value) || Date.now() > deadline) return value;
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
}

describe.skipIf(!url)("live node through the gateway", () => {
  const client = new MayaClient(url ?? "");
  const sender = env.MAYA_SENDER ?? "";
  const recipient = env.MAYA_RECIPIENT ?? "";
  const amount = Number(env.MAYA_AMOUNT);

  it("submits a Rust-signed transfer and sees it finalized", async () => {
    await expect(client.health()).resolves.toEqual({ status: "ok" });
    const before = await client.getBalance(sender);
    const recipientBefore = await client.getBalance(recipient);
    const supply = await client.getSupply();
    expect(before.balance).toBeGreaterThan(amount);

    const started = Date.now();
    const accepted = await client.sendRawTransaction(env.MAYA_RAW_TX ?? "");
    expect(accepted.hash).toMatch(/^[0-9a-f]{64}$/);

    const after = await until(
      () => client.getBalance(recipient),
      (b) => b.balance === recipientBefore.balance + amount,
    );
    const elapsed = Date.now() - started;
    expect(after.balance).toBe(recipientBefore.balance + amount);
    const senderAfter = await client.getBalance(sender);
    expect(senderAfter.nonce).toBe(before.nonce + 1);
    // Fees are burned or paid out, so supply can only fall, never rise.
    expect((await client.getSupply()).total).toBeLessThanOrEqual(supply.total);
    console.log(`sdk e2e: submitted via gateway, recipient credited after ${elapsed} ms`);

    // Replaying the same bytes must not pay twice, and the refusal must be a
    // 400: `retryable` is `status >= 500`, so a 502 here would have a client
    // resubmit a stale-nonce transaction forever.
    const replay = await client.sendRawTransaction(env.MAYA_RAW_TX ?? "").catch((e: unknown) => e);
    expect(replay).toBeInstanceOf(GatewayError);
    expect((replay as GatewayError).status).toBe(400);
    expect((replay as GatewayError).retryable).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 3_000));
    expect((await client.getBalance(recipient)).balance).toBe(after.balance);
  }, FINALITY_WAIT_MS + 30_000);

  it("reads a finalized block and refuses a malformed transaction", async () => {
    await expect(client.getBlock(0)).resolves.toBeTruthy();
    const bad = await client.sendRawTransaction("zz").catch((e: unknown) => e);
    expect(bad).toBeInstanceOf(GatewayError);
    expect((bad as GatewayError).status).toBe(400);
  });
});
