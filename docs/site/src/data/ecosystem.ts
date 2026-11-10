// The ecosystem roadmap, read from docs/ecosystem/CATALOG.md and COMPETITORS.md
// at build time. Nothing here is copied by hand: a row added to a table in the
// docs appears on the page on the next build. A table whose header or cell
// values change fails the build instead of rendering a wrong page, which is the
// same rule scripts/check-fields.mjs applies to the fields page.
//
// The docs folder is found by walking up from the working directory, not from
// this file: the build bundles this module into dist/, so a path relative to
// the source file points nowhere (the first build failed that way).
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

function ecosystemDir(): string {
  let dir = process.cwd();
  for (;;) {
    const candidate = resolve(dir, "docs/ecosystem");
    if (existsSync(resolve(candidate, "CATALOG.md"))) return candidate;
    const up = dirname(dir);
    if (up === dir) throw new Error("ecosystem: docs/ecosystem/CATALOG.md not found above the working directory");
    dir = up;
  }
}
const read = (name: string) => readFileSync(resolve(ecosystemDir(), name), "utf8");

const REPO_ECOSYSTEM = "https://github.com/EricWijesinghe/Maya2C/blob/master/docs/ecosystem/";

export const WAVES = ["W0", "W1", "W2", "W3", "W4"] as const;
export type Wave = (typeof WAVES)[number];
export const VERDICTS = ["Lead", "Par", "Trail", "n/a"] as const;
export type Verdict = (typeof VERDICTS)[number];
export const RUNGS = ["R0", "R1", "R2"] as const;
export type Rung = (typeof RUNGS)[number];
export type Size = "S" | "M" | "L";
/** open: the gate says "none". decision: an owner call. review: an ADR, review or audit. */
export type GateKind = "open" | "decision" | "review";

export interface Entry {
  id: string;
  section: string;
  sectionName: string;
  what: string;
  builds: string;
  gate: string;
  gateKind: GateKind;
  size: Size;
  wave: Wave;
}

export interface Horizon {
  id: string;
  need: string;
  rung: Rung;
  builds: string;
  /** Plain-text note from the doc's last column, links removed. */
  note: string;
  /** Link to the prior-art record, when the doc links one. */
  priorArt: string | null;
  /** True when the doc says the prior-art search has not been run. */
  searchPending: boolean;
}

export interface Matrix {
  chains: { code: string; name: string }[];
  rows: { dimension: string; cells: Verdict[] }[];
  tally: Record<Verdict, number>;
}

export interface Closure {
  cell: string;
  closedBy: string;
  /** Entry ids named in "Closed by" that exist in the catalog. */
  ids: string[];
  notes: string;
}

export interface Ecosystem {
  waves: { id: Wave; when: string; rule: string }[];
  sections: { key: string; name: string }[];
  entries: Entry[];
  horizon: Horizon[];
  matrix: Matrix;
  closures: Closure[];
  links: { catalog: string; competitors: string };
}

type Table = { header: string[]; rows: string[][] };

/** Full names for the chain codes in COMPETITORS.md. An unknown code is shown as-is. */
const CHAIN_NAMES: Record<string, string> = {
  BTC: "Bitcoin", ETH: "Ethereum", SOL: "Solana", SUI: "Sui", APT: "Aptos",
  ALGO: "Algorand", QRL: "QRL", ADA: "Cardano", DOT: "Polkadot", XMR: "Monero",
  ZEC: "Zcash", MINA: "Mina", TIA: "Celestia",
};

const ENTRY_HEADER = ["id", "What, for whom", "Builds on", "Gate", "Size", "Wave"];
const HORIZON_HEADER = ["id", "Need (inspired by)", "Rung", "Builds on", "Known prior art / gate"];

/** Markdown inline syntax to plain text: links keep their label. */
export function plain(text: string): string {
  return text
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Which gate column text needs what. Heuristic on the doc's own wording:
 * "none" is open; a mention of an owner, approval, budget or directive is an
 * owner call; anything else is a review. The card shows the gate text in full.
 */
export function gateKind(gate: string): GateKind {
  if (/^none\b/i.test(gate)) return "open";
  if (/\b(Eric|owner|approval|budget|directive|decision|memory)\b/i.test(gate)) return "decision";
  return "review";
}

function cellsOf(line: string): string[] {
  return line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim());
}

/** The first pipe table under the heading that matches `title`. Throws on a malformed table. */
function tableUnder(md: string, title: RegExp, file: string): Table {
  const lines = md.split("\n");
  const start = lines.findIndex((l) => title.test(l));
  if (start < 0) throw new Error(`${file}: no heading matches ${title}`);
  let i = start + 1;
  while (i < lines.length && !lines[i].startsWith("|")) {
    if (/^#{1,6} /.test(lines[i])) throw new Error(`${file}: heading ${title} has no table`);
    i += 1;
  }
  const header = cellsOf(lines[i] ?? "");
  i += 1;
  if (!/^\|[\s:|-]+\|?$/.test(lines[i] ?? "")) throw new Error(`${file}: ${title} table has no separator row`);
  i += 1;
  const rows: string[][] = [];
  for (; i < lines.length && lines[i].startsWith("|"); i += 1) {
    const cells = cellsOf(lines[i]);
    if (cells.length !== header.length) {
      throw new Error(`${file}: row "${cells[0]}" under ${title} has ${cells.length} cells, header has ${header.length}`);
    }
    rows.push(cells);
  }
  return { header, rows };
}

function expectHeader(t: Table, want: string[], where: string): void {
  if (t.header.join("|") !== want.join("|")) {
    throw new Error(`${where}: header is [${t.header.join(", ")}], expected [${want.join(", ")}]`);
  }
}

function asWave(value: string, where: string): Wave {
  if ((WAVES as readonly string[]).includes(value)) return value as Wave;
  throw new Error(`${where}: wave "${value}" is not one of ${WAVES.join(", ")}`);
}

function asId(value: string, where: string): string {
  // Lower-case, digits, hyphens and dots: "sdks-1.0" is a real id.
  if (/^[a-z0-9][a-z0-9.-]*$/.test(value)) return value;
  throw new Error(`${where}: id "${value}" is not a lower-case slug`);
}

/** Named sections A to I, from the `## X. Name` headings, with the parenthetical dropped. */
function sectionHeadings(md: string): { key: string; name: string }[] {
  return [...md.matchAll(/^## ([A-I])\. (.+)$/gm)].map((m) => ({
    key: m[1],
    name: plain(m[2]).replace(/\s*\(.*\)$/, ""),
  }));
}

function parseEntries(md: string, file: string, sections: { key: string; name: string }[]): Entry[] {
  const out: Entry[] = [];
  const seen = new Set<string>();
  // A to F are the wave-scheduled tables. G is a fact list and H has no wave
  // column; I is the horizon table, parsed separately.
  for (const { key, name } of sections.filter((s) => s.key <= "F")) {
    const t = tableUnder(md, new RegExp(`^## ${key}\\. `), file);
    expectHeader(t, ENTRY_HEADER, `${file} section ${key}`);
    for (const [id, what, builds, gate, size, wave] of t.rows) {
      asId(id, file);
      if (seen.has(id)) throw new Error(`${file}: duplicate id ${id}`);
      seen.add(id);
      if (size !== "S" && size !== "M" && size !== "L") throw new Error(`${file}: ${id} size "${size}" is not S, M or L`);
      out.push({
        id,
        section: key,
        sectionName: name,
        what: plain(what),
        builds: plain(builds),
        gate: plain(gate),
        gateKind: gateKind(plain(gate)),
        size,
        wave: asWave(wave, `${file}: ${id}`),
      });
    }
  }
  return out;
}

function parseWaves(md: string, file: string): Ecosystem["waves"] {
  const t = tableUnder(md, /^## Waves$/, file);
  expectHeader(t, ["Wave", "When", "Rule"], `${file} waves`);
  if (t.rows.length !== WAVES.length) throw new Error(`${file}: expected ${WAVES.length} waves, found ${t.rows.length}`);
  return t.rows.map(([w, when, rule]) => ({ id: asWave(w, `${file} waves`), when: plain(when), rule: plain(rule) }));
}

function parseHorizon(md: string, file: string): Horizon[] {
  const t = tableUnder(md, /^## I\. /, file);
  expectHeader(t, HORIZON_HEADER, `${file} section I`);
  return t.rows.map(([id, need, rung, builds, prior]) => {
    if (!(RUNGS as readonly string[]).includes(rung)) throw new Error(`${file}: ${id} rung "${rung}" is not R0, R1 or R2`);
    const link = /\]\((\.\.\/prior-art\/[^)\s]+)\)/.exec(prior);
    return {
      id: asId(id, file),
      need: plain(need),
      rung: rung as Rung,
      builds: plain(builds),
      note: plain(prior),
      priorArt: link ? new URL(link[1], REPO_ECOSYSTEM).href : null,
      searchPending: /not yet searched/i.test(prior),
    };
  });
}

function parseMatrix(md: string, file: string): Matrix {
  const t = tableUnder(md, /^## 3\. /, file);
  if (t.header[0] !== "Dimension" || t.header.length < 2) throw new Error(`${file}: verdict matrix header is malformed`);
  const tally = Object.fromEntries(VERDICTS.map((v) => [v, 0])) as Record<Verdict, number>;
  const rows = t.rows.map((cells) => {
    const verdicts = cells.slice(1).map((c) => {
      if (!(VERDICTS as readonly string[]).includes(c)) throw new Error(`${file}: verdict "${c}" under "${cells[0]}" is not Lead, Par, Trail or n/a`);
      tally[c as Verdict] += 1;
      return c as Verdict;
    });
    return { dimension: plain(cells[0]), cells: verdicts };
  });
  const chains = t.header.slice(1).map((code) => ({ code, name: CHAIN_NAMES[code] ?? code }));
  return { chains, rows, tally };
}

function parseClosures(md: string, file: string, knownIds: Set<string>): Closure[] {
  const t = tableUnder(md, /^## 4\. /, file);
  expectHeader(t, ["Trail cell", "Closed by", "Notes"], `${file} section 4`);
  return t.rows.map(([cell, closedBy, notes]) => ({
    cell: plain(cell),
    closedBy: plain(closedBy),
    ids: [...closedBy.matchAll(/`([a-z0-9-]+)`/g)].map((m) => m[1]).filter((id) => knownIds.has(id)),
    notes: plain(notes),
  }));
}

/** Everything the roadmap page shows, read from the two docs. */
export function loadEcosystem(): Ecosystem {
  const catalogFile = "CATALOG.md";
  const competitorsFile = "COMPETITORS.md";
  const catalog = read("CATALOG.md");
  const competitors = read("COMPETITORS.md");

  const sections = sectionHeadings(catalog);
  const entries = parseEntries(catalog, catalogFile, sections);
  if (entries.length === 0) throw new Error(`${catalogFile}: no wave entries parsed`);
  const known = new Set(entries.map((e) => e.id));

  return {
    waves: parseWaves(catalog, catalogFile),
    sections: sections.filter((s) => s.key <= "F"),
    entries,
    horizon: parseHorizon(catalog, catalogFile),
    matrix: parseMatrix(competitors, competitorsFile),
    closures: parseClosures(competitors, competitorsFile, known),
    links: { catalog: `${REPO_ECOSYSTEM}CATALOG.md`, competitors: `${REPO_ECOSYSTEM}COMPETITORS.md` },
  };
}
