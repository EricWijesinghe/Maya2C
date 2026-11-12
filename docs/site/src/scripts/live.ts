// One client for the public network, shared by every live widget on a page.
// Each figure is read from a named endpoint and nothing else: if an endpoint
// does not answer, the caller gets `null` and shows a dash (Standing Order 1).
// All hosts below are already in the CSP connect-src (astro.config.mjs).
import { COIN_DECIMALS } from "../lib/ui-kit.ts";

export const EXPLORER = "https://explorer.maya2c.dev";
export const RPC = "https://rpc.maya2c.dev/rpc";
export const STATUS = "https://status.maya2c.dev";
const TIMEOUT_MS = 8000;

export type Block = { height: number; timestamp: number; tx_count: number };
export type ExplorerStats = { height: number; transactions_indexed: number; hashrate: number; average_block_time: number };
export type Supply = { total: string; circulating: string; burned: string; shielded?: string };
export type Bft = { committee: number; committed_round: number };

async function json<T>(url: string, init?: RequestInit): Promise<T | null> {
  try {
    const r = await fetch(url, { cache: "no-store", signal: AbortSignal.timeout(TIMEOUT_MS), ...init });
    return r.ok ? ((await r.json()) as T) : null;
  } catch {
    return null;
  }
}

export async function rpc<T>(method: string): Promise<T | null> {
  const body = JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: [] });
  const r = await json<{ result?: T }>(RPC, { method: "POST", headers: { "content-type": "application/json" }, body });
  return r?.result ?? null;
}

export const explorerStats = () => json<ExplorerStats>(`${EXPLORER}/api/stats`);
export const recentBlocks = (limit = 30) => json<Block[]>(`${EXPLORER}/api/blocks?limit=${limit}`);
export const supply = () => rpc<Supply>("get_supply");
export const bft = () => rpc<Bft>("get_bft_status");
export const uptime = () => json<{ uptime?: number; halted_now?: boolean }>(`${STATUS}/api/status`);
export const peers = () => json<{ peers?: unknown[] }>(`${STATUS}/api/live`);

/** Pushes every new block from the explorer's WebSocket; reconnects with backoff. */
export function onBlocks(cb: (b: Block) => void, onState?: (live: boolean) => void): void {
  let retry = 1000;
  const open = () => {
    let ws: WebSocket;
    try { ws = new WebSocket(`${EXPLORER.replace(/^http/, "ws")}/ws/blocks`); } catch { return; }
    ws.onopen = () => { retry = 1000; onState?.(true); };
    ws.onmessage = (ev) => {
      try {
        const b = JSON.parse(ev.data);
        if (typeof b.height === "number" && typeof b.timestamp === "number") cb({ height: b.height, timestamp: b.timestamp, tx_count: b.tx_count ?? 0 });
      } catch { /* malformed frame skipped */ }
    };
    ws.onclose = () => { onState?.(false); setTimeout(open, retry); retry = Math.min(retry * 2, 30000); };
    ws.onerror = () => ws.close();
  };
  open();
}

export const fmt = (n: number) => n.toLocaleString("en-US");

export function hashFmt(h: number): string {
  const u = ["H/s", "kH/s", "MH/s", "GH/s", "TH/s", "PH/s"];
  let i = 0;
  while (h >= 1000 && i < u.length - 1) { h /= 1000; i++; }
  return `${h.toFixed(2)} ${u[i]}`;
}

/** Blocks whose header timestamp is within `seconds` of the newest one (header time, not the visitor's clock). */
export function blocksWithin(blocks: Block[], seconds: number): number {
  if (!blocks.length) return 0;
  const newest = Math.max(...blocks.map((b) => b.timestamp));
  return blocks.filter((b) => newest - b.timestamp < seconds).length;
}

/** Exact share of a 256-bit decimal string over another, in basis points of a percent (0..10000), via BigInt. */
export function shareBp(part: string, whole: string): number | null {
  if (!/^\d+$/.test(part) || !/^\d+$/.test(whole) || BigInt(whole) === 0n) return null;
  return Number((BigInt(part) * 10000n) / BigInt(whole));
}

/** A visitor's share of the network if they add `mine` H/s to `network` H/s. */
export function hashShare(mine: number, network: number): number | null {
  if (!(mine > 0) || !(network > 0) || !Number.isFinite(mine + network)) return null;
  return mine / (network + mine);
}

/** Whole coins from base units, rounded down, grouped. */
export function coins(units: string, decimals = COIN_DECIMALS): string | null {
  if (!/^\d+$/.test(units)) return null;
  return (BigInt(units) / 10n ** BigInt(decimals)).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}
