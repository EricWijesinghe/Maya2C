// The Live network page's browser half: fetches, the 2.5D ring and its pulses.
//
// Standing Order 3: a pulse is drawn only for a block or checkpoint the
// network actually reported (the explorer's block stream, or a height that
// rose between two polls). Offline, nothing moves. `?demo` replays a fixed
// sample and says DEMO on screen the whole time it runs.
import {
  type Block, type Consensus, type Finality, type NetState, type Validator,
  layoutRing, mapBlock, mapBlocks, mapConsensus, mapFinality, mapLive, mapStakeValidators,
  mergeBlocks, networkState, timeline,
} from "./live-network";

const EXPLORER = "https://explorer.maya2c.dev";
const STATUS = "https://status.maya2c.dev";
const RPC = "https://rpc.maya2c.dev/rpc";
const POLL_MS = 15000;
const FETCH_TIMEOUT_MS = 8000;
const LAST_SEEN_KEY = "maya2c.network.lastSeen";

type View = {
  validators: Validator[]; blocks: Block[]; consensus: Consensus | null; finality: Finality | null;
  reachedAny: boolean; polled: boolean; demo: boolean;
};

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const fmt = (n: number | null | undefined) => (n === null || n === undefined ? "MISSING" : n.toLocaleString("en-US"));
const reduce = window.matchMedia("(prefers-reduced-motion: reduce)");
const view: View = { validators: [], blocks: [], consensus: null, finality: null, reachedAny: false, polled: false, demo: false };

async function getJson(url: string, init?: RequestInit): Promise<unknown> {
  const r = await fetch(url, { cache: "no-store", signal: AbortSignal.timeout(FETCH_TIMEOUT_MS), ...init });
  if (!r.ok) throw new Error(`${url}: ${r.status}`);
  return r.json();
}
async function rpc(method: string): Promise<unknown> {
  const body = JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: [] });
  const j = await getJson(RPC, { method: "POST", headers: { "content-type": "application/json" }, body });
  return (j as { result?: unknown } | null)?.result ?? null;
}
const settle = async <T>(p: Promise<T>): Promise<T | undefined> => p.catch(() => undefined);

function readLastSeen(): { at: number; height: number } | null {
  try {
    const v = JSON.parse(localStorage.getItem(LAST_SEEN_KEY) ?? "null");
    return Number.isSafeInteger(v?.at) && Number.isSafeInteger(v?.height) ? v : null;
  } catch { return null; }
}
function writeLastSeen(height: number) {
  try { localStorage.setItem(LAST_SEEN_KEY, JSON.stringify({ at: Date.now(), height })); } catch { /* private window: no memory, still correct */ }
}
const ago = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 120 ? `${s} s ago` : s < 7200 ? `${Math.round(s / 60)} min ago` : `${Math.round(s / 3600)} h ago`;
};

// --- fetching ---------------------------------------------------------------

async function poll() {
  const [live, stake, blocks, bft, cp] = await Promise.all([
    settle(getJson(`${STATUS}/api/live`).then(mapLive)),
    settle(getJson(`${EXPLORER}/api/validators`).then(mapStakeValidators)),
    settle(getJson(`${EXPLORER}/api/blocks`).then(mapBlocks)),
    settle(rpc("get_bft_status").then(mapConsensus)),
    settle(rpc("get_checkpoint").then(mapFinality)),
  ]);
  const before = { tip: view.blocks[0]?.height ?? null, final: view.finality?.height ?? null };
  view.polled = true;
  view.reachedAny = [live, stake, blocks, bft, cp].some((x) => x !== undefined);
  // Watched nodes say who is up; the stake record is the fallback, drawn as liveness unknown.
  view.validators = live?.length ? live : stake?.validators ?? [];
  view.blocks = blocks ? mergeBlocks(view.blocks, blocks) : [];
  view.consensus = bft ?? (stake ? { epoch: stake.epoch, committee: stake.committee, round: null, committedRound: null } : null);
  view.finality = cp ?? null;
  const tip = view.blocks[0]?.height ?? null;
  if (tip !== null && before.tip !== null && tip > before.tip) pulse("block");
  if (view.finality && before.final !== null && view.finality.height > before.final) pulse("final");
  if (tip !== null) writeLastSeen(tip);
  render();
}

function stream() {
  let ws: WebSocket;
  try { ws = new WebSocket(`${EXPLORER.replace(/^http/, "ws")}/ws/blocks`); } catch { return; }
  ws.onmessage = (ev) => {
    let b: Block | null = null;
    try { b = mapBlock(JSON.parse(String(ev.data))); } catch { /* malformed frame: skipped, never shown */ }
    if (!b || (view.blocks[0] && b.height <= view.blocks[0].height)) return;
    view.blocks = mergeBlocks(view.blocks, [b]);
    view.reachedAny = true;
    writeLastSeen(b.height);
    pulse("block");
    render();
  };
  ws.onclose = () => setTimeout(stream, POLL_MS);
}

// --- demo: a fixed sample, labelled DEMO on screen ----------------------------

function startDemo() {
  view.demo = true;
  const now = Math.floor(Date.now() / 1000);
  view.validators = ["v1", "v2", "v3", "v4", "v5", "v6", "v7"].map((name, i) => ({
    name, up: i !== 5, voting: i !== 5 && i !== 6, follower: i === 6, height: i === 5 ? null : 1200 - (i === 6 ? 3 : 0),
    lagRounds: i === 6 ? 4 : 0, source: "status" as const,
  }));
  view.blocks = Array.from({ length: 12 }, (_, i) => ({ height: 1200 - i, time: now - i * 6, txs: (i * 7) % 5, id: `demo${i}` }));
  view.consensus = { epoch: 12, committee: 7, round: 2401, committedRound: 2399 };
  view.finality = { height: 1197, epoch: 12, signatures: 5 };
  view.reachedAny = true;
  view.polled = true;
  render();
  setInterval(() => {
    const top = view.blocks[0];
    view.blocks = mergeBlocks(view.blocks, [{ height: top.height + 1, time: Math.floor(Date.now() / 1000), txs: top.height % 4, id: `demo${top.height + 1}` }]);
    pulse("block");
    if (view.blocks[0].height % 3 === 0 && view.finality) {
      view.finality = { ...view.finality, height: view.blocks[0].height - 2 };
      pulse("final");
    }
    render();
  }, 6000);
}

// --- DOM ----------------------------------------------------------------------

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls: string, txt = ""): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  e.className = cls;
  e.textContent = txt;
  return e;
}

function nodeExplain(v: Validator): string {
  if (v.source === "stake") return `${v.name}…: in this epoch's committee by the stake record (${v.voting ? "bonded" : "not bonded"}); no public monitor watches it, so whether it is up is unknown.`;
  if (!v.up) return `${v.name}: did not answer the status monitor's last poll.`;
  if (v.follower) return `${v.name}: up and following quorum-attested blocks after a catch-up, not voting yet (ADR-038).`;
  return `${v.name}: up${v.voting ? " and voting" : ", not holding a seat"}; height ${fmt(v.height)}, ${fmt(v.lagRounds)} rounds behind the furthest node.`;
}
function nodeClass(v: Validator): string {
  if (v.up === null) return "unknown";
  if (!v.up) return "down";
  return v.follower ? "follower" : v.voting ? "voting" : "idle";
}

// Rebuilding a list must not throw away a keyboard user's place in it.
function replaceKeepingFocus(list: HTMLElement, items: HTMLElement[]) {
  const key = (document.activeElement as HTMLElement | null)?.dataset?.key;
  list.replaceChildren(...items);
  if (key) list.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`)?.focus();
}

function renderNodes(state: NetState) {
  const list = $("ln-nodes");
  const pts = layoutRing(view.validators.length);
  replaceKeepingFocus(list, view.validators.map((v, i) => {
    const li = el("li", "ln-node-li");
    const b = el("button", `ln-node ${state === "offline" ? "down" : nodeClass(v)}`);
    b.type = "button";
    b.dataset.key = `v:${v.name}`;
    b.dataset.explain = state === "offline" ? `${v.name}: offline with the rest of the view.` : nodeExplain(v);
    b.setAttribute("aria-label", `Validator ${v.name}, ${nodeClass(v)}`);
    b.style.left = `${pts[i].x * 100}%`;
    b.style.top = `${pts[i].y * 100}%`;
    b.style.setProperty("--s", String(pts[i].scale));
    b.append(el("span", "ln-node-name", v.name));
    li.append(b);
    return li;
  }));
}

function renderTimeline() {
  const strip = $("ln-timeline");
  const pos = timeline(view.blocks);
  const maxTx = Math.max(1, ...view.blocks.map((b) => b.txs));
  const final = view.finality?.height ?? -1;
  replaceKeepingFocus(strip, view.blocks.map((b, i) => {
    const btn = el("button", `ln-block${b.height <= final ? " final" : ""}`);
    btn.type = "button";
    btn.dataset.key = `b:${b.height}`;
    btn.style.left = `${4 + pos[i].at * 92}%`;
    btn.style.setProperty("--h", String(0.3 + 0.7 * (b.txs / maxTx)));
    btn.setAttribute("aria-label", `Block ${b.height}`);
    btn.dataset.explain = `Block ${fmt(b.height)}: ${b.txs} transaction${b.txs === 1 ? "" : "s"}, ${ago(Date.now() - b.time * 1000)}${b.height <= final ? ", finalized" : ""}.`;
    return btn;
  }));
  $("ln-timeline-empty").hidden = view.blocks.length > 0;
}

function render() {
  const tip = view.blocks[0];
  const state = view.demo ? "live" : networkState(view.reachedAny, tip?.time ?? null, Date.now() / 1000);
  const root = $("ln");
  root.dataset.state = view.demo ? "demo" : view.polled ? state : "connecting";
  const last = readLastSeen();
  const banner: Record<NetState, string> = {
    live: "Live: maya-testnet-1, read from the public explorer, gateway and status monitor.",
    stalled: `Reachable, but no new block for ${tip ? ago(Date.now() - tip.time * 1000).replace(" ago", "") : "a while"}.`,
    partial: "Partly reachable: some sources answered, the block list did not.",
    offline: `Offline: no public endpoint answered. ${last ? `Last seen at height ${fmt(last.height)}, ${ago(Date.now() - last.at)} (this browser).` : "Not seen from this browser before."}`,
  };
  $("ln-banner").textContent = view.demo
    ? "DEMO: sample data, not the network. Nothing here is live."
    : view.polled ? banner[state] : "Connecting to the public explorer, gateway and status monitor…";
  const off = state === "offline";
  // Offline, a dash: there is no answer to be missing from. Reachable, MISSING: that source left it out.
  const show = (n: number | null | undefined) => (off ? "—" : fmt(n));
  $("ln-height-label").textContent = off && view.polled ? "last seen" : "height";
  $("ln-height").textContent = off ? (last && view.polled ? fmt(last.height) : "—") : tip ? fmt(tip.height) : "MISSING";
  $("ln-final").textContent = show(view.finality?.height);
  $("ln-sigs").textContent = view.finality ? `${view.finality.signatures} signatures` : "";
  $("ln-epoch").textContent = show(view.consensus?.epoch);
  $("ln-committee").textContent = show(view.consensus?.committee);
  $("ln-round").textContent = show(view.consensus?.committedRound);
  $("ln-age").textContent = off ? "—" : tip ? ago(Date.now() - tip.time * 1000) : "MISSING";
  renderNodes(state);
  renderTimeline();
  drawLinks(state);
  playPulses();
}

// --- SVG ring and links, HTML sparks ---------------------------------------------
// The SVG is in stage fractions (0..100 both ways, stretched to the stage), the
// same frame layoutRing uses, and takes the theme's colours straight from CSS:
// no redraw on resize or theme change. A block's pulse is a spark (an HTML dot)
// that CSS moves from the tip to each voting validator, so it stays round at
// any stage shape.
const SVG_NS = "http://www.w3.org/2000/svg";
let pending = new Set<"block" | "final">();

function line(cls: string, x: number, y: number): SVGLineElement {
  const l = document.createElementNS(SVG_NS, "line");
  const attrs: [string, string][] = [["x1", "50"], ["y1", "50"], ["x2", String(x * 100)], ["y2", String(y * 100)], ["class", cls]];
  for (const [k, v] of attrs) l.setAttribute(k, v);
  return l;
}

function drawLinks(state: NetState) {
  const pts = layoutRing(view.validators.length);
  $("ln-links").replaceChildren(...view.validators.flatMap((v, i) => {
    const dim = state === "offline" || v.up === false;
    return [line(`ln-link${dim ? " dim" : ""}`, pts[i].x, pts[i].y)];
  }));
}

// Restarts a CSS animation: drop the class, flush style, add it back.
function replay(e: Element) {
  e.classList.remove("go");
  void e.getBoundingClientRect();
  e.classList.add("go");
}

/** Queues a pulse for an event the network reported; render() plays it after it rebuilds the links. */
function pulse(kind: "block" | "final") {
  if (!reduce.matches) pending.add(kind); // static fallback: numbers update, nothing travels
}
function playPulses() {
  if (pending.has("block")) {
    const pts = layoutRing(view.validators.length);
    const sparks = view.validators.flatMap((v, i) => {
      if (!v.voting || v.up === false) return [];
      const s = el("span", "ln-spark");
      s.style.setProperty("--x", `${pts[i].x * 100}%`);
      s.style.setProperty("--y", `${pts[i].y * 100}%`);
      s.addEventListener("animationend", () => s.remove());
      return [s];
    });
    $("ln-sparks").replaceChildren(...sparks);
  }
  if (pending.has("final")) replay($("ln-ring-flash"));
  pending = new Set();
}

// --- wiring ---------------------------------------------------------------------

export function start() {
  const root = document.getElementById("ln");
  if (!root) return;
  root.addEventListener("click", (e) => {
    const t = (e.target as HTMLElement).closest<HTMLElement>("[data-explain]");
    if (t) $("ln-explain").textContent = t.dataset.explain ?? "";
  });
  setInterval(() => { if (!view.demo) render(); }, POLL_MS / 3); // ages tick; no data is invented between polls

  if (new URLSearchParams(location.search).has("demo")) {
    startDemo();
    return;
  }
  render();
  poll();
  setInterval(poll, POLL_MS);
  stream();
}
