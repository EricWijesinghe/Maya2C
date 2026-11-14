/**
 * Background (Chrome service worker / Firefox event page): wires browser APIs
 * to `WalletService`. Decisions live in the service; this file only routes.
 *
 * The unlocked seed lives in this context's memory only. When the browser
 * stops the worker, or after 15 minutes without use, the wallet locks —
 * deliberately: nothing secret is ever written to `storage.session` or disk
 * unencrypted.
 */
import init, * as wasm from "../wasm/maya_sdk_wasm.js";
import { MayaClient } from "../../../sdks/sdk-js/src/index.js";
import { parseCoins } from "./lib/bytes.js";
import { routeOf, toReply, WalletService, type Store } from "./lib/service.js";

declare const chrome: any;
const api: any = (globalThis as any).browser ?? chrome;

export const DEFAULT_GATEWAY = "http://127.0.0.1:8080";
/** The person types this to erase the wallet. */
const ERASE_PHRASE = "ERASE";

const store: Store = {
  async get(key) {
    return (await api.storage.local.get(key))[key];
  },
  async set(key, value) {
    await api.storage.local.set({ [key]: value });
  },
  async remove(key) {
    await api.storage.local.remove(key);
  },
};

const ready = init({ module_or_path: api.runtime.getURL("wasm/maya_sdk_wasm_bg.wasm") });

/** A gateway client that refuses redirects: an https gateway cannot bounce the wallet elsewhere. */
async function gateway(): Promise<MayaClient> {
  const url = (await store.get<string>("gateway")) ?? DEFAULT_GATEWAY;
  return new MayaClient(url, { fetch: (input, init) => fetch(input, { ...init, redirect: "error" }) });
}

let approvalWindow: number | undefined;

function openApproval(): void {
  // One approval window: focus it if it is open, rather than stacking more.
  const create = () =>
    api.windows
      .create({ url: api.runtime.getURL("popup.html"), type: "popup", width: 380, height: 640 })
      .then((w: { id: number }) => (approvalWindow = w.id));
  const shown =
    approvalWindow === undefined
      ? create()
      : api.windows.update(approvalWindow, { focused: true }).catch(() => create());
  Promise.resolve(shown).catch(() => service.rejectAll());
}

const service = new WalletService({ wasm, store, gateway, openApproval });

// Closing the approval window answers every waiting request with "no".
api.windows.onRemoved.addListener((id: number) => {
  if (id === approvalWindow) {
    approvalWindow = undefined;
    service.rejectAll();
  }
});

// The popup holds a port while it is open, so the worker is not stopped
// under a confirm screen the person is reading.
api.runtime.onConnect.addListener((port: any) => {
  if (port.name !== "popup" || !String(port.sender?.url ?? "").startsWith(api.runtime.getURL(""))) port.disconnect();
});

type PopupMsg = { cmd: string; [k: string]: unknown };

async function popup(msg: PopupMsg): Promise<unknown> {
  switch (msg.cmd) {
    case "status":
      return {
        hasVault: await service.hasVault(),
        unlocked: service.unlocked,
        pending: service.pending(),
        last: service.lastResult,
        gateway: (await store.get<string>("gateway")) ?? DEFAULT_GATEWAY,
        origins: await service.connectedOrigins(),
      };
    case "create":
      return { mnemonic: await service.create(String(msg.password)) };
    case "import":
      await service.importMnemonic(String(msg.mnemonic), String(msg.passphrase ?? ""), String(msg.password));
      return null;
    case "unlock":
      await service.unlock(String(msg.password));
      return null;
    case "lock":
      service.lock();
      return null;
    case "erase":
      if (msg.confirm !== ERASE_PHRASE) throw new Error(`type ${ERASE_PHRASE} to erase this wallet`);
      await service.erase();
      return null;
    case "account": {
      const gw = await gateway();
      const id = service.address();
      const [chain, balance] = await Promise.all([gw.getChain(), gw.getBalance(id)]);
      return { id, chain, balance };
    }
    case "planSend": {
      const plan = await service.planTransfer(String(msg.to), parseCoins(String(msg.amount)));
      // Settled from the approval screen; the outcome shows as `last` in status.
      service.requestTransfer(plan, null).catch(() => undefined);
      return null;
    }
    case "settle":
      service.settle(String(msg.id), msg.approve === true);
      return null;
    case "setGateway": {
      const url = new URL(String(msg.url));
      if (url.protocol !== "https:" && url.hostname !== "127.0.0.1" && url.hostname !== "localhost") {
        throw new Error("a gateway must be https, or http on this machine");
      }
      await store.set("gateway", url.origin);
      return null;
    }
    case "disconnect":
      await service.disconnect(String(msg.origin));
      return null;
    default:
      throw new Error("unknown command");
  }
}

api.runtime.onMessage.addListener((msg: any, sender: any, reply: (r: unknown) => void) => {
  const route = routeOf(sender, api.runtime.id, api.runtime.getURL(""));
  if (route.kind === "refuse" || typeof msg !== "object" || msg === null) {
    // Answer rather than stay silent, so a refused page's promise settles.
    reply({ ok: false, code: 4100, message: "this page may not use the wallet" });
    return false;
  }
  // Only the person using the popup keeps the wallet unlocked; a page calling
  // in a loop must not hold the seed in memory past the inactivity lock.
  if (route.kind === "popup") service.touch();
  toReply(
    ready.then(() => (route.kind === "popup" ? popup(msg) : service.provider(route.origin, String(msg.method), msg.params))),
  ).then(reply);
  return true;
});
