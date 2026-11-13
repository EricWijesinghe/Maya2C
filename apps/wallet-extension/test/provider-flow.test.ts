// End-to-end-ish: a page's `window.maya2c.request` -> content relay ->
// background routing (sender check) -> WalletService -> approval -> real wasm
// signature -> mock gateway. Only the browser and the network are doubles.
import { beforeEach, describe, expect, it } from "vitest";
import { wasm } from "./wasm.js";
import { createProvider } from "../src/lib/provider.js";
import { installRelay } from "../src/lib/relay.js";
import { routeOf, toReply, WalletService, FEE_COLLECTOR_HEX, type Gateway, type Seed, type Store, type Wasm } from "../src/lib/service.js";
import { encodeAddress } from "../../../sdks/sdk-js/src/address.js";
import { fromHex } from "../src/lib/bytes.js";

const PHRASE = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const GENESIS = "5a".repeat(32);
const EXT_ID = "walletext";
const EXT_BASE = "chrome-extension://walletext/";

/** A window double: postMessage dispatches to its own listeners, as a page's does. */
function fakeWindow(origin: string): Window {
  const target = new EventTarget() as unknown as Window & EventTarget;
  Object.assign(target, {
    location: { origin },
    postMessage(data: unknown) {
      queueMicrotask(() => target.dispatchEvent(Object.assign(new Event("message"), { data, origin, source: target })));
    },
  });
  return target;
}

function memoryStore(): Store {
  const m = new Map<string, unknown>();
  return {
    get: async (k) => structuredClone(m.get(k)) as never,
    set: async (k, v) => void m.set(k, structuredClone(v)),
    remove: async (k) => void m.delete(k),
  };
}

function mockGateway(state: { genesis: string; nonce: number; submitted: string[] }): Gateway {
  return {
    getChain: async () => ({ genesis: state.genesis, chain_id: "maya-testnet-1", height: 10 }),
    getFees: async () => ({ active: true, base_fee: 2, collector: FEE_COLLECTOR_HEX }),
    getBalance: async () => ({ balance: (10n ** 30n).toString(), nonce: state.nonce }),
    sendRawTransaction: async (raw) => {
      state.submitted.push(raw);
      return { hash: "ab".repeat(32) };
    },
  };
}

describe("dapp provider permission flow", () => {
  let service: WalletService;
  let state: { genesis: string; nonce: number; submitted: string[] };
  let approvals: number;

  beforeEach(async () => {
    state = { genesis: GENESIS, nonce: 7, submitted: [] };
    approvals = 0;
    service = new WalletService({
      wasm,
      store: memoryStore(),
      gateway: async () => mockGateway(state),
      openApproval: () => void approvals++,
    });
    await service.importMnemonic(PHRASE, "", "correct horse");
  });

  /** A page at `url`, wired through the relay to the background router. */
  function page(url: string, frameId = 0) {
    const origin = new URL(url).origin;
    const win = fakeWindow(origin);
    installRelay(win, (msg) => {
      // What the browser would stamp on the content script's message.
      const route = routeOf({ id: EXT_ID, url, origin, tab: {}, frameId }, EXT_ID, EXT_BASE);
      if (route.kind !== "provider") return Promise.resolve({ ok: false, code: 4100, message: "refused" });
      return toReply(service.provider(route.origin, msg.method, msg.params));
    });
    return createProvider(win);
  }

  async function nextApproval() {
    for (let i = 0; i < 200 && service.pending().length === 0; i++) await new Promise((r) => setTimeout(r, 5));
    return service.pending()[0];
  }

  it("shows nothing and signs nothing before the person connects the origin", async () => {
    const dapp = page("https://dapp.example/app");
    await expect(dapp.request({ method: "accounts" })).resolves.toEqual([]);
    await expect(
      dapp.request({ method: "sendTransfer", params: { to: "11".repeat(32), amount: "1", genesis: GENESIS } }),
    ).rejects.toMatchObject({ code: 4100 });
    expect(approvals).toBe(0);
  });

  it("connects on approval, for that origin only", async () => {
    const dapp = page("https://dapp.example/app");
    const connecting = dapp.request({ method: "connect" });
    const approval = await nextApproval();
    expect(approval).toMatchObject({ kind: "connect", origin: "https://dapp.example" });
    service.settle(approval.id, true);
    const address = wasm.WalletSeed.fromMnemonic(PHRASE, "").addressAt(0, 0);
    await expect(connecting).resolves.toEqual([address]);
    await expect(dapp.request({ method: "accounts" })).resolves.toEqual([address]);
    // Another origin, and an iframe of the granted one, still see nothing.
    await expect(page("https://other.example/").request({ method: "accounts" })).resolves.toEqual([]);
    await expect(page("https://dapp.example/frame", 2).request({ method: "accounts" })).rejects.toMatchObject({ code: 4100 });
  });

  it("returns 4001 when the person rejects, and grants nothing", async () => {
    const dapp = page("https://dapp.example/");
    const connecting = dapp.request({ method: "connect" });
    service.settle((await nextApproval()).id, false);
    await expect(connecting).rejects.toMatchObject({ code: 4001 });
    await expect(dapp.request({ method: "accounts" })).resolves.toEqual([]);
  });

  it("makes a site wait after the person rejects it, longer each time", async () => {
    const svc = new WalletService({
      wasm, store: memoryStore(), gateway: async () => mockGateway(state), openApproval: () => void approvals++, rejectCooldownMs: 40,
    });
    await svc.importMnemonic(PHRASE, "", "correct horse");
    const ask = () => svc.provider("https://spam.example", "connect", null);
    const waitFor = async () => {
      for (let i = 0; i < 200 && svc.pending().length === 0; i++) await new Promise((r) => setTimeout(r, 5));
      return svc.pending()[0];
    };
    const first = ask();
    svc.settle((await waitFor()).id, false);
    await expect(first).rejects.toMatchObject({ code: 4001 });
    const before = approvals;
    // Inside the cooldown: refused without a prompt, and the window is not refocused.
    await expect(ask()).rejects.toThrow(/wait before asking again/);
    expect(approvals).toBe(before);
    // Another site is not affected.
    const other = svc.provider("https://other.example", "connect", null);
    svc.settle((await waitFor()).id, false);
    await expect(other).rejects.toMatchObject({ code: 4001 });
    await new Promise((r) => setTimeout(r, 60));
    const second = ask();
    svc.settle((await waitFor()).id, false);
    await expect(second).rejects.toMatchObject({ code: 4001 });
    // The second cooldown is doubled (80 ms): still refused after 60 ms.
    await new Promise((r) => setTimeout(r, 60));
    await expect(ask()).rejects.toThrow(/wait before asking again/);
    svc.lock();
  });

  it("unlocking keeps the requests that waited for it", async () => {
    service.lock();
    const connecting = service.provider("https://dapp.example", "connect", null);
    const approval = await nextApproval();
    await service.unlock("correct horse");
    service.settle(approval.id, true);
    await expect(connecting).resolves.toEqual([service.address()]);
  });

  it("refuses a plain-http, non-loopback page outright", async () => {
    await expect(page("http://dapp.example/").request({ method: "connect" })).rejects.toMatchObject({ code: 4100 });
    expect(approvals).toBe(0);
  });

  it("signs the transfer shown on the confirm screen, and refuses one for another chain", async () => {
    const dapp = page("https://dapp.example/");
    const c = dapp.request({ method: "connect" });
    service.settle((await nextApproval()).id, true);
    await c;

    // An invoice for another chain is refused before any confirm screen.
    await expect(
      dapp.request({ method: "sendTransfer", params: { to: "11".repeat(32), amount: "5", genesis: "00".repeat(32) } }),
    ).rejects.toMatchObject({ code: -32602 });
    // A mainnet-prefixed address on this test chain is refused (#446).
    await expect(
      dapp.request({ method: "sendTransfer", params: { to: encodeAddress("mainnet", new Uint8Array(32).fill(0x11)), amount: "5", genesis: GENESIS } }),
    ).rejects.toThrow(/mainnet/);
    expect(service.pending()).toHaveLength(0);

    const to = encodeAddress("testnet", new Uint8Array(32).fill(0x11));
    const sending = dapp.request({ method: "sendTransfer", params: { to, amount: "5", genesis: GENESIS } });
    const approval = await nextApproval();
    if (approval.kind !== "transfer") throw new Error("expected a transfer approval");
    expect(approval.plan).toMatchObject({
      toId: "11".repeat(32), amount: "5", nonce: 7, genesis: GENESIS, network: "test", feeTo: FEE_COLLECTOR_HEX,
    });
    expect(BigInt(approval.plan.fee)).toBe(2n * BigInt(wasm.transferWireLen(true)));
    // The confirm screen's words come from sdk-wasm, bound to the signing hash.
    const shown = approval.plan.confirmation;
    expect(Object.isFrozen(shown)).toBe(true);
    expect(shown.recipient.replace(/ /g, "")).toBe(encodeAddress("devnet", new Uint8Array(32).fill(0x11)));
    expect(shown.description).toContain("Send 5 base units of the native coin to dmaya1");
    expect(shown.signingHash).toMatch(/^[0-9a-f]{64}$/);
    service.settle(approval.id, true);
    await expect(sending).resolves.toEqual({ hash: "ab".repeat(32) });

    const raw = fromHex(state.submitted[0]);
    expect(raw.length).toBe(wasm.transferWireLen(true));
    // Re-sign the plan locally: the submitted bytes are exactly that plan,
    // and the hash the confirm screen showed is the one they were signed for.
    const seed = wasm.WalletSeed.fromMnemonic(PHRASE, "");
    const le = (v: bigint) => Uint8Array.from({ length: 32 }, (_, i) => Number((v >> BigInt(8 * i)) & 0xffn));
    const expected = seed.signTransfer(0, 0, fromHex("11".repeat(32)), le(5n), le(BigInt(approval.plan.fee)),
      fromHex(FEE_COLLECTOR_HEX), 7n, fromHex(GENESIS), shown.signingHash);
    expect(raw).toEqual(expected);
  });

  it("signs nothing when the plan's signing hash does not match its transfer", async () => {
    // A wasm whose confirm screen reports a hash for other bytes: the signer
    // recomputes the hash from the plan and refuses, so a confirm screen that
    // drifted from what is signed can never release a signature.
    const lying: Wasm = { ...wasm, WalletSeed: { fromMnemonic: (phrase: string, passphrase: string): Seed => {
      const seed = wasm.WalletSeed.fromMnemonic(phrase, passphrase);
      return {
        addressAt: (a, i) => seed.addressAt(a, i),
        signTransfer: (...a) => seed.signTransfer(...a),
        free: () => seed.free(),
        confirmTransfer: (...a) => {
          const real = seed.confirmTransfer(...a);
          return { signingHash: "00".repeat(32), description: real.description, recipient: real.recipient,
            sender: real.sender, addressPrefix: real.addressPrefix, warnings: real.warnings };
        },
      };
    } } };
    const svc = new WalletService({ wasm: lying, store: memoryStore(), gateway: async () => mockGateway(state), openApproval: () => undefined });
    await svc.importMnemonic(PHRASE, "", "correct horse");
    const plan = await svc.planTransfer("11".repeat(32), 5n, GENESIS);
    const sending = svc.requestTransfer(plan, null);
    for (let i = 0; i < 200 && svc.pending().length === 0; i++) await new Promise((r) => setTimeout(r, 5));
    svc.settle(svc.pending()[0].id, true);
    await expect(sending).rejects.toThrow(/nothing was signed/);
    expect(state.submitted).toHaveLength(0);
  });

  it("locks itself after the inactivity timeout and then shows and signs nothing", async () => {
    const quick = new WalletService({
      wasm, store: memoryStore(), gateway: async () => mockGateway(state), openApproval: () => undefined, autoLockMs: 30,
    });
    await quick.importMnemonic(PHRASE, "", "correct horse battery");
    expect(quick.unlocked).toBe(true);
    await new Promise((r) => setTimeout(r, 80));
    expect(quick.unlocked).toBe(false);
    expect(() => quick.address()).toThrow(/locked/);
    await expect(quick.planTransfer("11".repeat(32), 5n, GENESIS)).rejects.toMatchObject({ code: 4100 });
    // The vault is still there and opens again with the password.
    await quick.unlock("correct horse battery");
    expect(quick.unlocked).toBe(true);
    quick.lock();
  });

  it("does not sign if the chain changed while the confirm screen was open", async () => {
    const dapp = page("https://dapp.example/");
    const c = dapp.request({ method: "connect" });
    service.settle((await nextApproval()).id, true);
    await c;
    const sending = dapp.request({ method: "sendTransfer", params: { to: "11".repeat(32), amount: "5", genesis: GENESIS } });
    const approval = await nextApproval();
    state.genesis = "77".repeat(32);
    service.settle(approval.id, true);
    await expect(sending).rejects.toMatchObject({ code: -32603 });
    expect(state.submitted).toHaveLength(0);
    expect(service.lastResult).toMatchObject({ ok: false, message: expect.stringMatching(/chain changed/) });
  });

  it("locking rejects what is waiting, and nothing is approved while locked", async () => {
    const dapp = page("https://dapp.example/");
    const c = dapp.request({ method: "connect" });
    const approval = await nextApproval();
    service.lock();
    await expect(c).rejects.toMatchObject({ code: 4001 });
    expect(() => service.settle(approval.id, true)).toThrow(/no longer waiting/);
    await expect(dapp.request({ method: "accounts" })).resolves.toEqual([]);
  });

  it("allows one waiting request per site and caps the total", async () => {
    const dapp = page("https://dapp.example/");
    const first = dapp.request({ method: "connect" });
    await nextApproval();
    await expect(dapp.request({ method: "connect" })).rejects.toMatchObject({ code: 4001 });
    const others = [1, 2, 3, 4].map((i) => page(`https://site${i}.example/`).request({ method: "connect" }));
    for (let i = 0; i < 200 && service.pending().length < 5; i++) await new Promise((r) => setTimeout(r, 5));
    expect(service.pending()).toHaveLength(5);
    await expect(page("https://site9.example/").request({ method: "connect" })).rejects.toMatchObject({ code: 4001 });
    service.rejectAll();
    await expect(first).rejects.toMatchObject({ code: 4001 });
    for (const o of others) await expect(o).rejects.toMatchObject({ code: 4001 });
  });

  it("expires a request nobody answers", async () => {
    const quick = new WalletService({
      wasm, store: memoryStore(), gateway: async () => mockGateway(state), openApproval: () => undefined, approvalTtlMs: 30,
    });
    await quick.importMnemonic(PHRASE, "", "correct horse battery");
    await expect(quick.provider("https://dapp.example", "connect", null)).rejects.toMatchObject({ code: 4001 });
    expect(quick.pending()).toHaveLength(0);
  });

  it("disconnecting withdraws the grant and rejects that site's waiting transfer", async () => {
    const dapp = page("https://dapp.example/");
    const c = dapp.request({ method: "connect" });
    service.settle((await nextApproval()).id, true);
    await c;
    const sending = dapp.request({ method: "sendTransfer", params: { to: "11".repeat(32), amount: "5", genesis: GENESIS } });
    await nextApproval();
    await service.disconnect("https://dapp.example");
    await expect(sending).rejects.toMatchObject({ code: 4001 });
    await expect(dapp.request({ method: "accounts" })).resolves.toEqual([]);
    expect(state.submitted).toHaveLength(0);
  });

  it("does not sign if the account's nonce moved while the confirm screen was open", async () => {
    const dapp = page("https://dapp.example/");
    const c = dapp.request({ method: "connect" });
    service.settle((await nextApproval()).id, true);
    await c;
    const sending = dapp.request({ method: "sendTransfer", params: { to: "11".repeat(32), amount: "5", genesis: GENESIS } });
    const approval = await nextApproval();
    state.nonce = 8;
    service.settle(approval.id, true);
    await expect(sending).rejects.toMatchObject({ code: -32603 });
    expect(state.submitted).toHaveLength(0);
    expect(service.lastResult).toMatchObject({ ok: false, message: expect.stringMatching(/account changed/) });
  });

  it("tells a site nothing about the balance or the gateway in its errors", async () => {
    const dapp = page("https://dapp.example/");
    const c = dapp.request({ method: "connect" });
    service.settle((await nextApproval()).id, true);
    await c;
    await expect(
      dapp.request({ method: "sendTransfer", params: { to: "11".repeat(32), amount: (10n ** 31n).toString(), genesis: GENESIS } }),
    ).rejects.toMatchObject({ code: -32603, message: "the wallet could not complete the request" });
  });
});
