/**
 * The wallet's background logic, free of browser APIs so it can be tested:
 * vault, unlock state, per-origin grants, pending approvals, and the transfer
 * plan a confirm screen shows and the signer signs — the same object, so what
 * the person approved is what gets signed.
 */
import type { ChainInfo, FeeInfo, Balance, TransactionAccepted } from "../../../../sdks/sdk-js/src/index.js";
import { formatCoins, fromHex, parseBaseUnits, toHex, toLe32, wipe } from "./bytes.js";
import { classify, displayName, fingerprint, parseRecipient, type NetworkClass } from "./network.js";
import { accountsFor, grant, grantableOrigin, revoke, type Grants } from "./permissions.js";
import { openVault, sealVault, type VaultRecord } from "./vault.js";

/**
 * sdk-wasm's `WalletSeed`: the seed lives in wasm memory, never in a JS
 * buffer, and `free()` wipes it there.
 */
export interface Seed {
  addressAt(account: number, index: number): string;
  /** The clear-sign rendering and signing hash, from sdk-wasm (the desktop's renderer). */
  confirmTransfer(
    account: number, index: number, recipient: Uint8Array, amount: Uint8Array,
    fee: Uint8Array, feeRecipient: Uint8Array, nonce: bigint, chain: Uint8Array, balance: Uint8Array,
  ): WasmConfirmation;
  /** Signs only the transfer whose signing hash is `confirmedHash`. */
  signTransfer(
    account: number, index: number, recipient: Uint8Array, amount: Uint8Array,
    fee: Uint8Array, feeRecipient: Uint8Array, nonce: bigint, chain: Uint8Array, confirmedHash: string,
  ): Uint8Array;
  free(): void;
}

/** The sdk-wasm exports this service uses (wasm-bindgen names). */
export interface Wasm {
  mnemonicFromEntropy(entropy: Uint8Array): string;
  validateMnemonic(phrase: string): void;
  WalletSeed: { fromMnemonic(phrase: string, passphrase: string): Seed };
  transferWireLen(withFee: boolean): number;
  vaultKey(password: Uint8Array, salt: Uint8Array): Uint8Array;
}

/** sdk-wasm's `TransferConfirmation` (a wasm-bindgen object; `free` releases it). */
export interface WasmConfirmation {
  readonly signingHash: string;
  readonly description: string;
  readonly recipient: string;
  readonly sender: string;
  readonly addressPrefix: string;
  readonly warnings: string[];
  free?(): void;
}

/** What the confirm screen shows, rendered in Rust from the bytes that will be signed. */
export interface Confirmation {
  /** BLAKE3 of the signing bytes: the signer signs nothing else. */
  readonly signingHash: string;
  readonly description: string;
  readonly recipient: string;
  readonly sender: string;
  readonly addressPrefix: string;
  readonly warnings: readonly string[];
}

/** The gateway calls this service makes (a subset of sdk-js `MayaClient`). */
export interface Gateway {
  getChain(): Promise<ChainInfo>;
  getFees(): Promise<FeeInfo>;
  getBalance(address: string): Promise<Balance>;
  sendRawTransaction(raw: string): Promise<TransactionAccepted>;
}

export interface Store {
  get<T>(key: string): Promise<T | undefined>;
  set(key: string, value: unknown): Promise<void>;
  remove(key: string): Promise<void>;
}

export interface Deps {
  wasm: Wasm;
  store: Store;
  gateway: () => Promise<Gateway>;
  /** Brings the approval window up (or to the front, if already open). */
  openApproval: () => void;
  /** Milliseconds a site must wait after the person rejects, or lets expire, its request; doubles on repeats. */
  rejectCooldownMs?: number;
  /** Milliseconds before an approval nobody answered is rejected. */
  approvalTtlMs?: number;
  /** Milliseconds of inactivity before the wallet locks itself. */
  autoLockMs?: number;
}

/** Provider error codes, numbered as EIP-1193 numbers them so dapp code ports. */
export const REJECTED = 4001;
export const UNAUTHORIZED = 4100;
export const UNSUPPORTED = 4200;
export const INVALID_PARAMS = -32602;
export const INTERNAL = -32603;

export class ProviderError extends Error {
  constructor(readonly code: number, message: string) {
    super(message);
    this.name = "ProviderError";
  }
}

/** `FEE_COLLECTOR` from crates/node/src/state/fees.rs: "maya2c/fee/collector/v1", zero-padded. */
export const FEE_COLLECTOR_HEX = toHex(
  Uint8Array.from({ length: 32 }, (_, i) => "maya2c/fee/collector/v1".charCodeAt(i) || 0),
);
/** The desktop wallet's fee cap without an override: one coin. */
export const MAX_FEE = 10n ** 24n;
/** At most this many requests wait for the person at once, across all sites. */
export const MAX_PENDING = 5;
const DEFAULT_TTL_MS = 2 * 60_000;
const DEFAULT_AUTO_LOCK_MS = 15 * 60_000;
const DEFAULT_COOLDOWN_MS = 30_000;
/** The longest a site's cooldown grows to after repeated rejections. */
const MAX_COOLDOWN_MS = 10 * 60_000;

/** Everything the confirm screen shows, and everything the signer signs. */
export interface TransferPlan {
  readonly from: string;
  readonly toId: string;
  readonly toNamedNetwork: string | null;
  readonly amount: string;
  readonly fee: string;
  readonly feeTo: string;
  readonly nonce: number;
  readonly genesis: string;
  readonly network: NetworkClass;
  readonly networkName: string;
  readonly fingerprint: string;
  readonly confirmation: Confirmation;
}

type Approval =
  | { id: string; kind: "connect"; origin: string; settle: (ok: boolean) => void }
  | { id: string; kind: "transfer"; origin: string | null; plan: TransferPlan; settle: (ok: boolean) => void };

/** What the popup lists: an approval without its callbacks. */
export type ApprovalView =
  | { id: string; kind: "connect"; origin: string }
  | { id: string; kind: "transfer"; origin: string | null; plan: TransferPlan; amountCoins: string; feeCoins: string };

/** The outcome of the last transfer the person approved, for the popup. */
export type LastResult = { ok: true; hash: string } | { ok: false; message: string } | null;

/** Who sent a runtime message, as the browser reports it. */
export interface SenderInfo {
  id?: string;
  url?: string;
  origin?: string;
  frameId?: number;
  tab?: unknown;
}

/**
 * Routes a runtime message by who the *browser* says sent it. Popup-only
 * commands (unlock, approve) are accepted only from this extension's own
 * pages; a content script's message is a provider call whose origin is the
 * page's, and only from a top-level frame, so an embedded iframe cannot borrow
 * its parent's grant.
 */
export function routeOf(
  sender: SenderInfo,
  extensionId: string,
  extensionBase: string,
): { kind: "popup" } | { kind: "provider"; origin: string } | { kind: "refuse" } {
  if (sender.id !== extensionId) return { kind: "refuse" };
  const url = sender.url ?? "";
  if (url.startsWith(extensionBase)) return { kind: "popup" };
  if (sender.tab === undefined || sender.frameId !== 0) return { kind: "refuse" };
  const origin = grantableOrigin(sender.origin ?? url);
  return origin ? { kind: "provider", origin } : { kind: "refuse" };
}

/** The reply envelope the content script and popup receive. */
export type Reply = { ok: true; result: unknown } | { ok: false; code: number; message: string };

const messageOf = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/** Settles a call into a reply; errors keep their provider code, others are -32603. */
export function toReply(work: Promise<unknown>): Promise<Reply> {
  return work.then(
    (result) => ({ ok: true as const, result }),
    (e: unknown) => ({ ok: false as const, code: e instanceof ProviderError ? e.code : INTERNAL, message: messageOf(e) }),
  );
}

const VAULT_KEY = "vault";
const GRANTS_KEY = "grants";

export class WalletService {
  #seed: Seed | null = null;
  #pending = new Map<string, Approval>();
  /** Per site: no new prompt before `until`; `ms` is the next cooldown's length. */
  #cooldown = new Map<string, { until: number; ms: number }>();
  #last: LastResult = null;
  #lockTimer: ReturnType<typeof setTimeout> | undefined;
  // Serialises vault and grant read-modify-writes: two clicks, or two sites,
  // must not interleave a check with a write.
  #queue: Promise<unknown> = Promise.resolve();

  constructor(private readonly deps: Deps) {}

  #serial<T>(work: () => Promise<T>): Promise<T> {
    const next = this.#queue.then(work, work);
    this.#queue = next.catch(() => undefined);
    return next;
  }

  get unlocked(): boolean {
    return this.#seed !== null;
  }

  get lastResult(): LastResult {
    return this.#last;
  }

  /** Restarts the inactivity timer; called on every popup and provider call. */
  touch(): void {
    clearTimeout(this.#lockTimer);
    if (this.#seed) this.#lockTimer = setTimeout(() => this.lock(), this.deps.autoLockMs ?? DEFAULT_AUTO_LOCK_MS);
  }

  async hasVault(): Promise<boolean> {
    return (await this.deps.store.get<VaultRecord>(VAULT_KEY)) !== undefined;
  }

  /** Creates a 24-word wallet; returns the words once, for the backup screen. */
  async create(password: string): Promise<string> {
    const entropy = crypto.getRandomValues(new Uint8Array(32));
    try {
      const mnemonic = this.deps.wasm.mnemonicFromEntropy(entropy);
      await this.importMnemonic(mnemonic, "", password);
      return mnemonic;
    } finally {
      wipe(entropy);
    }
  }

  importMnemonic(mnemonic: string, passphrase: string, password: string): Promise<void> {
    return this.#serial(async () => {
      if (await this.hasVault()) throw new Error("a wallet already exists; erase it before importing another");
      const phrase = mnemonic.trim().split(/\s+/).join(" ").toLowerCase();
      this.deps.wasm.validateMnemonic(phrase);
      const record = await sealVault(this.deps.wasm.vaultKey, JSON.stringify({ m: phrase, p: passphrase }), password);
      await this.deps.store.set(VAULT_KEY, record);
      this.#unlockWith(phrase, passphrase);
    });
  }

  async unlock(password: string): Promise<void> {
    const record = await this.deps.store.get<VaultRecord>(VAULT_KEY);
    if (!record) throw new Error("no wallet");
    const opened = JSON.parse(await openVault(this.deps.wasm.vaultKey, record, password)) as { m?: unknown; p?: unknown };
    if (typeof opened.m !== "string" || typeof opened.p !== "string") throw new Error("the vault is damaged");
    this.#unlockWith(opened.m, opened.p);
  }

  /** Erases the vault and every grant. The person typed the confirmation in the popup. */
  erase(): Promise<void> {
    return this.#serial(async () => {
      this.lock();
      await this.deps.store.remove(VAULT_KEY);
      await this.deps.store.remove(GRANTS_KEY);
      this.#last = null;
    });
  }

  #unlockWith(phrase: string, passphrase: string): void {
    // Not lock(): that would reject the connect requests that waited for this unlock.
    this.#seed?.free();
    this.#seed = this.deps.wasm.WalletSeed.fromMnemonic(phrase, passphrase);
    this.touch();
  }

  /** Locks, wipes the seed in wasm memory, and rejects every request waiting for the person. */
  lock(): void {
    clearTimeout(this.#lockTimer);
    this.#seed?.free();
    this.#seed = null;
    this.rejectAll();
  }

  #requireSeed(): Seed {
    if (!this.#seed) throw new ProviderError(UNAUTHORIZED, "the wallet is locked");
    return this.#seed;
  }

  /** The hex account id of account 0. Locked wallets have none to give. */
  address(): string {
    return this.#requireSeed().addressAt(0, 0);
  }

  async #grants(): Promise<Grants> {
    return (await this.deps.store.get<Grants>(GRANTS_KEY)) ?? {};
  }

  async connectedOrigins(): Promise<string[]> {
    return Object.keys(await this.#grants());
  }

  /** Withdraws a grant, and rejects anything that origin still has waiting. */
  disconnect(origin: string): Promise<void> {
    for (const a of [...this.#pending.values()]) if (a.origin === origin) this.#drop(a.id, false);
    return this.#serial(async () => this.deps.store.set(GRANTS_KEY, revoke(await this.#grants(), origin)));
  }

  // ---- approvals -------------------------------------------------------

  /**
   * Throws unless `origin` may put a prompt in front of the person now: one
   * request per site at a time, a few in total, and a growing pause after the
   * person rejects one or lets it expire, so a page cannot bury the person in
   * prompts (or refocus the window in a loop) until one is clicked by accident.
   * Called before any planning work too, so a refused page costs nothing.
   */
  #mayAsk(origin: string | null): void {
    const waiting = [...this.#pending.values()];
    if (origin !== null && waiting.some((a) => a.origin === origin)) {
      throw new ProviderError(REJECTED, "this site already has a request waiting");
    }
    if (waiting.length >= MAX_PENDING) throw new ProviderError(REJECTED, "too many requests waiting");
    const pause = origin === null ? undefined : this.#cooldown.get(origin);
    if (pause && Date.now() < pause.until) {
      throw new ProviderError(REJECTED, "this site's last request was declined; wait before asking again");
    }
  }

  #ask(origin: string | null, make: (id: string, settle: (ok: boolean) => void) => Approval): Promise<boolean> {
    try {
      this.#mayAsk(origin);
    } catch (e) {
      return Promise.reject(e);
    }
    return new Promise((resolve) => {
      const id = crypto.randomUUID();
      this.#pending.set(id, make(id, resolve));
      setTimeout(() => this.#drop(id, false, true), this.deps.approvalTtlMs ?? DEFAULT_TTL_MS);
      this.deps.openApproval();
    });
  }

  /** `penalize`: the person rejected it or let it expire, so the site waits before asking again. */
  #drop(id: string, ok: boolean, penalize = false): void {
    const approval = this.#pending.get(id);
    if (!approval) return;
    this.#pending.delete(id);
    if (approval.origin !== null) {
      if (ok) {
        this.#cooldown.delete(approval.origin);
      } else if (penalize) {
        const base = this.deps.rejectCooldownMs ?? DEFAULT_COOLDOWN_MS;
        const ms = Math.min(this.#cooldown.get(approval.origin)?.ms ?? base, MAX_COOLDOWN_MS);
        this.#cooldown.set(approval.origin, { until: Date.now() + ms, ms: Math.min(ms * 2, MAX_COOLDOWN_MS) });
      }
    }
    approval.settle(ok);
  }

  /** Rejects every waiting request: on lock, and when the approval window closes. */
  rejectAll(): void {
    for (const id of [...this.#pending.keys()]) this.#drop(id, false);
  }

  pending(): ApprovalView[] {
    return [...this.#pending.values()].map((a) =>
      a.kind === "connect"
        ? { id: a.id, kind: a.kind, origin: a.origin }
        : { id: a.id, kind: a.kind, origin: a.origin, plan: a.plan,
            amountCoins: formatCoins(BigInt(a.plan.amount)), feeCoins: formatCoins(BigInt(a.plan.fee)) },
    );
  }

  /** Settles an approval. Approving needs an unlocked wallet. */
  settle(id: string, approve: boolean): void {
    if (!this.#pending.has(id)) throw new Error("that request is no longer waiting");
    if (approve) this.#requireSeed();
    this.#drop(id, approve, !approve);
  }

  // ---- transfers -------------------------------------------------------

  /**
   * Builds the plan for a transfer: network classified by genesis, recipient
   * checked against that network, fee quoted and capped, balance checked.
   * `expectedGenesis` is the chain an invoice or dapp says it is for; a
   * different chain is refused, never silently switched.
   */
  async planTransfer(to: string, amount: bigint, expectedGenesis?: string): Promise<TransferPlan> {
    const from = this.address();
    const gw = await this.deps.gateway();
    const chain = await gw.getChain();
    const genesis = String(chain.genesis ?? "").trim().toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(genesis)) throw new Error("the gateway reported a malformed genesis id");
    if (expectedGenesis !== undefined && expectedGenesis.trim().toLowerCase() !== genesis) {
      throw new ProviderError(INVALID_PARAMS,
        `this request is for chain ${fingerprint(expectedGenesis)}, but the wallet is connected to ${fingerprint(genesis)}; refusing`);
    }
    const network = classify(chain.chain_id, genesis);
    let recipient;
    try {
      recipient = parseRecipient(to, network);
    } catch (e) {
      throw new ProviderError(INVALID_PARAMS, messageOf(e));
    }
    if (amount <= 0n) throw new ProviderError(INVALID_PARAMS, "amount must be above zero");

    const fees = await gw.getFees();
    let fee = 0n;
    if (fees.active) {
      if (String(fees.collector).toLowerCase().replace(/^0x/, "") !== FEE_COLLECTOR_HEX) {
        throw new Error("the gateway named a fee collector that is not this chain's; refusing to pay a fee to it");
      }
      if (!Number.isSafeInteger(fees.base_fee) || fees.base_fee < 0) throw new Error("the gateway quoted a malformed fee");
      fee = BigInt(fees.base_fee) * BigInt(this.deps.wasm.transferWireLen(true));
    }
    if (fee > MAX_FEE) throw new Error("the quoted fee is above the 1 MAYA cap; nothing was signed");

    const account = await this.#account(gw, from);
    if (amount + fee > account.balance) throw new Error("the balance does not cover amount plus fee");
    const feeTo = fee > 0n ? FEE_COLLECTOR_HEX : "00".repeat(32);

    return Object.freeze({
      from,
      toId: toHex(recipient.id),
      toNamedNetwork: recipient.named,
      amount: amount.toString(),
      fee: fee.toString(),
      feeTo,
      nonce: account.nonce,
      genesis,
      network,
      networkName: displayName(genesis),
      fingerprint: fingerprint(genesis),
      confirmation: this.#confirm(recipient.id, amount, fee, feeTo, account, genesis),
    });
  }

  /** The words and hash the confirm screen shows, rendered by sdk-wasm, frozen. */
  #confirm(
    to: Uint8Array, amount: bigint, fee: bigint, feeTo: string, account: { balance: bigint; nonce: number }, genesis: string,
  ): Confirmation {
    const shown = this.#requireSeed().confirmTransfer(
      0, 0, to, toLe32(amount), toLe32(fee), fromHex(feeTo), BigInt(account.nonce),
      fromHex(genesis), toLe32(account.balance),
    );
    try {
      return Object.freeze({
        signingHash: shown.signingHash,
        description: shown.description,
        recipient: shown.recipient,
        sender: shown.sender,
        addressPrefix: shown.addressPrefix,
        warnings: Object.freeze([...shown.warnings]),
      });
    } finally {
      shown.free?.();
    }
  }

  async #account(gw: Gateway, address: string): Promise<{ balance: bigint; nonce: number }> {
    const a = await gw.getBalance(address);
    if (!Number.isSafeInteger(a.nonce) || a.nonce < 0) throw new Error("the gateway reported a malformed nonce");
    return { balance: parseBaseUnits(String(a.balance)), nonce: a.nonce };
  }

  /** Asks the person to approve `plan`, then signs exactly it and submits it. */
  async requestTransfer(plan: TransferPlan, origin: string | null): Promise<TransactionAccepted> {
    const ok = await this.#ask(origin, (id, settle) => ({ id, kind: "transfer", origin, plan, settle }));
    if (!ok) throw new ProviderError(REJECTED, "the transfer was rejected or expired");
    try {
      const accepted = await this.#signAndSubmit(plan);
      this.#last = { ok: true, hash: accepted.hash };
      return accepted;
    } catch (e) {
      this.#last = { ok: false, message: messageOf(e) };
      throw e;
    }
  }

  async #signAndSubmit(plan: TransferPlan): Promise<TransactionAccepted> {
    const gw = await this.deps.gateway();
    // The gateway could have been repointed, or the account moved, while the
    // confirm screen was open. Either way the reviewed plan no longer holds.
    if (String((await gw.getChain()).genesis).trim().toLowerCase() !== plan.genesis) {
      throw new Error("the connected chain changed after the confirm screen; nothing was signed");
    }
    const now = await this.#account(gw, plan.from);
    if (now.nonce !== plan.nonce || now.balance < BigInt(plan.amount) + BigInt(plan.fee)) {
      throw new Error("the account changed after the confirm screen; nothing was signed — review it again");
    }
    const raw = this.#requireSeed().signTransfer(
      0, 0, fromHex(plan.toId), toLe32(BigInt(plan.amount)), toLe32(BigInt(plan.fee)),
      fromHex(plan.feeTo), BigInt(plan.nonce), fromHex(plan.genesis), plan.confirmation.signingHash,
    );
    return gw.sendRawTransaction(toHex(raw));
  }

  // ---- the dapp provider -------------------------------------------------

  /**
   * One `window.maya2c.request` call from `origin` (already browser-verified).
   * Only `ProviderError`s reach the page with their text; anything else is
   * reported as a bare failure, so a site learns nothing about the gateway,
   * the balance or the wallet's internals from an error message.
   */
  async provider(origin: string, method: string, params: unknown): Promise<unknown> {
    try {
      return await this.#provider(origin, method, params);
    } catch (e) {
      if (e instanceof ProviderError) throw e;
      throw new ProviderError(INTERNAL, "the wallet could not complete the request");
    }
  }

  async #provider(origin: string, method: string, params: unknown): Promise<unknown> {
    const connected = accountsFor(await this.#grants(), origin).length > 0;
    switch (method) {
      case "connect": {
        if (!connected) {
          const ok = await this.#ask(origin, (id, settle) => ({ id, kind: "connect", origin, settle }));
          if (!ok) throw new ProviderError(REJECTED, "the request was declined or expired");
          await this.#serial(async () => this.deps.store.set(GRANTS_KEY, grant(await this.#grants(), origin, [0])));
        }
        return [this.address()];
      }
      case "accounts":
        return connected && this.unlocked ? [this.address()] : [];
      case "chain": {
        if (!connected) throw new ProviderError(UNAUTHORIZED, "connect first");
        const chain = await (await this.deps.gateway()).getChain();
        return { genesis: chain.genesis, network: classify(chain.chain_id, chain.genesis), name: displayName(chain.genesis) };
      }
      case "disconnect":
        await this.disconnect(origin);
        return null;
      case "sendTransfer": {
        if (!connected) throw new ProviderError(UNAUTHORIZED, "connect first");
        const p = params as { to?: unknown; amount?: unknown; genesis?: unknown } | null;
        if (!p || typeof p.to !== "string" || typeof p.amount !== "string" || typeof p.genesis !== "string") {
          throw new ProviderError(INVALID_PARAMS, "sendTransfer takes { to, amount (base units, decimal string), genesis }");
        }
        let amount: bigint;
        try {
          amount = parseBaseUnits(p.amount);
        } catch (e) {
          throw new ProviderError(INVALID_PARAMS, messageOf(e));
        }
        this.#mayAsk(origin);
        const plan = await this.planTransfer(p.to, amount, p.genesis);
        return this.requestTransfer(plan, origin);
      }
      default:
        throw new ProviderError(UNSUPPORTED, `unknown method ${JSON.stringify(method).slice(0, 64)}`);
    }
  }
}
