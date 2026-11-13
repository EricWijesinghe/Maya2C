/**
 * Popup and approval window. Every value from a gateway or a dapp is written
 * with `textContent`, never as HTML. The popup holds no key: it sends
 * commands, and the background signs only a plan it built and showed here.
 */
import { formatCoins, fromHex, parseBaseUnits } from "./lib/bytes.js";
import { displayAddress, displayName, fingerprint } from "./lib/network.js";
import type { ApprovalView, LastResult } from "./lib/service.js";
import { MIN_PASSWORD } from "./lib/vault.js";

declare const chrome: any;
const api: any = (globalThis as any).browser ?? chrome;
const app = document.getElementById("app") as HTMLElement;
const errorBox = document.getElementById("error") as HTMLElement;
/** Approve buttons stay disabled this long after a screen appears: no click lands on them by surprise. */
const APPROVE_DELAY_MS = 800;

// Held open while the popup is, so the background is not stopped mid-review.
api.runtime.connect({ name: "popup" });

async function call<T = unknown>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  const reply = await api.runtime.sendMessage({ cmd, ...args });
  if (!reply?.ok) throw new Error(reply?.message ?? "no reply from the wallet");
  return reply.result as T;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Record<string, unknown> = {}, ...kids: (Node | string)[]): HTMLElementTagNameMap[K] {
  const node: HTMLElementTagNameMap[K] = Object.assign(document.createElement(tag), props);
  node.append(...kids);
  return node;
}

const field = (label: string, input: HTMLInputElement | HTMLTextAreaElement) => el("label", {}, label, input);
const kv = (k: string, v: string, cls = "mono") => el("div", {}, el("div", { className: "k", textContent: k }), el("div", { className: cls, textContent: v }));

let busy = false;
/** Runs one action at a time; every button is inert while it runs, so a double click is one click. */
function act(fn: () => Promise<void>): () => void {
  return () => {
    if (busy) return;
    busy = true;
    errorBox.textContent = "";
    app.querySelectorAll("button").forEach((b) => (b.disabled = true));
    fn()
      .catch((e: Error) => (errorBox.textContent = e.message))
      .finally(() => {
        busy = false;
        app.querySelectorAll<HTMLButtonElement>("button:not([data-armed])").forEach((b) => (b.disabled = false));
      });
  };
}

function screen(...kids: Node[]): void {
  app.replaceChildren(...kids);
}

type Status = { hasVault: boolean; unlocked: boolean; pending: ApprovalView[]; last: LastResult; gateway: string; origins: string[] };

async function render(): Promise<void> {
  const s = await call<Status>("status");
  if (!s.hasVault) return renderSetup();
  if (!s.unlocked) return renderUnlock(s.pending.length);
  if (s.pending.length > 0) return renderApproval(s.pending[0], s.pending.length);
  return renderHome(s);
}

function renderSetup(): void {
  const pw = el("input", { type: "password", autocomplete: "new-password", minLength: MIN_PASSWORD });
  const words = el("textarea", { rows: 3, spellcheck: false, autocomplete: "off" });
  const passphrase = el("input", { type: "password", autocomplete: "off" });
  screen(
    el("p", { textContent: "Create a new wallet, or import 12/24 recovery words from the desktop wallet." }),
    field(`Password, at least ${MIN_PASSWORD} characters (encrypts the words on this device)`, pw),
    el("button", { textContent: "Create new wallet", onclick: act(async () => {
      const { mnemonic } = await call<{ mnemonic: string }>("create", { password: pw.value });
      pw.value = "";
      screen(
        el("p", { className: "warn", textContent: "Write these words down. They are the only backup, and they are shown once." }),
        el("div", { className: "card mono", textContent: mnemonic }),
        el("button", { textContent: "I have written them down", onclick: act(render) }),
      );
    }) }),
    el("hr"),
    field("Recovery words", words),
    field("BIP-39 passphrase (optional; empty on the desktop wallet unless you set one)", passphrase),
    el("button", { className: "secondary", textContent: "Import", onclick: act(async () => {
      await call("import", { mnemonic: words.value, passphrase: passphrase.value, password: pw.value });
      words.value = ""; passphrase.value = ""; pw.value = "";
      await render();
    }) }),
  );
}

function renderUnlock(waiting: number): void {
  const pw = el("input", { type: "password", autocomplete: "current-password" });
  const confirm = el("input", { autocomplete: "off", placeholder: "type ERASE" });
  screen(
    ...(waiting ? [el("p", { textContent: `${waiting} request(s) waiting. Unlock to review them.` })] : []),
    field("Password", pw),
    el("button", { textContent: "Unlock", onclick: act(async () => { await call("unlock", { password: pw.value }); pw.value = ""; await render(); }) }),
    el("details", {},
      el("summary", { textContent: "Forgot the password?" }),
      el("p", { textContent: "Erasing removes the encrypted words from this browser. You can import them again from your written backup. Without the backup, the coins are lost." }),
      field("Type ERASE to confirm", confirm),
      el("button", { className: "danger", textContent: "Erase this wallet", onclick: act(async () => { await call("erase", { confirm: confirm.value }); await render(); }) })),
  );
  pw.focus();
}

/** An approve button that arms only after a delay, and only while the window has focus. */
function armedButton(label: string, onclick: () => void): HTMLButtonElement {
  const button = el("button", { textContent: label, onclick, disabled: true });
  button.dataset.armed = "pending";
  setTimeout(() => {
    const arm = () => {
      if (document.hasFocus()) {
        delete button.dataset.armed;
        button.disabled = busy;
      } else {
        window.addEventListener("focus", arm, { once: true });
      }
    };
    arm();
  }, APPROVE_DELAY_MS);
  return button;
}

function renderApproval(a: ApprovalView, total: number): void {
  const settle = (approve: boolean) => act(async () => { await call("settle", { id: a.id, approve }); await render(); });
  const buttons = el("div", { className: "row" },
    el("button", { className: "secondary", textContent: "Reject", onclick: settle(false) }),
    armedButton(a.kind === "connect" ? "Connect" : "Sign and send", settle(true)));
  const counter = el("div", { className: "k", textContent: `Request 1 of ${total}` });
  if (a.kind === "connect") {
    return screen(counter, el("h2", { textContent: "Connect request" }), kv("Site", a.origin),
      el("p", { textContent: "This site will see your account address. It cannot move coins without asking you each time." }), buttons);
  }
  const p = a.plan;
  const c = p.confirmation;
  const warn = (text: string) => el("div", { className: "warn", textContent: text });
  // Every line about the transaction comes from the sdk-wasm rendering of the
  // bytes whose hash is shown last; the popup composes none of its own.
  screen(
    counter,
    el("h2", { textContent: "Confirm transfer" }),
    el("div", { className: "card" },
      ...c.warnings.map((w) => warn(`Warning: ${w}`)),
      kv("Network", `${p.networkName} — genesis ${p.fingerprint}`, "net"),
      ...(p.network === "mainnet" ? [] : [warn("This genesis is not pinned in this build as mainnet. Check the fingerprint against the one the network publishes.")]),
      kv("Requested by", a.origin ?? "you (this wallet)"),
      el("p", { textContent: c.description }),
      kv("Send", `${a.amountCoins} MAYA`, ""),
      kv("To", c.recipient),
      ...(p.toNamedNetwork === null ? [warn("A hex recipient names no network. Make sure it is meant for this chain.")] : []),
      kv("Fee", `${a.feeCoins} MAYA`, ""),
      kv("Total", `${formatCoins(BigInt(p.amount) + BigInt(p.fee))} MAYA`, ""),
      kv("From", c.sender),
      kv("Address prefix", c.addressPrefix, ""),
      kv("Nonce", String(p.nonce), ""),
      kv("Signing hash", c.signingHash)),
    buttons,
  );
}

function lastResultCard(last: LastResult): Node[] {
  if (!last) return [];
  return [last.ok
    ? el("div", { className: "card" }, kv("Last transfer accepted by the gateway", last.hash))
    : el("div", { className: "card" }, el("div", { className: "warn", textContent: `Last transfer not sent: ${last.message}` }))];
}

async function renderHome(s: Status): Promise<void> {
  const to = el("input", { spellcheck: false, autocomplete: "off", placeholder: "tmaya1… or 64 hex" });
  const amount = el("input", { inputMode: "decimal", placeholder: "1.5" });
  const gw = el("input", { value: s.gateway });
  const accountCard = el("div", { className: "card", textContent: "Reading the gateway…" });
  screen(
    accountCard,
    ...lastResultCard(s.last),
    el("h2", { textContent: "Send" }),
    field("Recipient", to),
    field("Amount (MAYA)", amount),
    el("button", { textContent: "Review", onclick: act(async () => { await call("planSend", { to: to.value, amount: amount.value }); await render(); }) }),
    el("h2", { textContent: "Settings" }),
    field("Gateway", gw),
    el("button", { className: "secondary", textContent: "Save gateway", onclick: act(async () => {
      const origin = new URL(gw.value).origin;
      // Host access is asked for per gateway, from this click, not up front.
      if (origin.startsWith("https:") && !(await api.permissions.request({ origins: [`${origin}/*`] }))) {
        throw new Error("the browser did not grant access to that gateway");
      }
      await call("setGateway", { url: gw.value });
      await render();
    }) }),
    ...s.origins.map((o) => el("div", { className: "row" }, el("span", { className: "mono", textContent: o }),
      el("button", { className: "secondary", textContent: "Disconnect", onclick: act(async () => { await call("disconnect", { origin: o }); await render(); }) }))),
    el("button", { className: "danger", textContent: "Lock", onclick: act(async () => { await call("lock"); await render(); }) }),
  );
  // Filled in afterwards, so a down or misconfigured gateway never hides the
  // settings that would fix it.
  try {
    const acct = await call<{ id: string; chain: { genesis: string; chain_id: string | null }; balance: { balance: string } }>("account");
    accountCard.replaceChildren(
      kv("Network", `${displayName(acct.chain.genesis)} — ${fingerprint(acct.chain.genesis)}`, "net"),
      kv("Address", displayAddress(acct.chain.genesis, fromHex(acct.id))),
      kv("Balance", `${formatCoins(parseBaseUnits(String(acct.balance.balance)))} MAYA`, ""));
  } catch (e) {
    accountCard.replaceChildren(el("div", { className: "warn", textContent: `Gateway not reachable: ${(e as Error).message}` }));
  }
}

act(render)();
