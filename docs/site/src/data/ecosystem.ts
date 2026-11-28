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

const REPO_ECOSYSTEM = "https://github.com/maya2c/Maya2C/blob/master/docs/ecosystem/";

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

/** A table row and its 1-based line in the source document, for error messages. */
type Row = { line: number; cells: string[] };
type Table = { header: string[]; headerLine: number; rows: Row[] };

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

/**
 * Where a parse error points: `docs/ecosystem/CATALOG.md:56`, a form editors
 * and terminals open directly. The build log is the only place this error is
 * read. From 2026-11-15 (run 43749589486, #418) it named neither the path nor
 * the line, and one mis-filled cell stopped every site deploy until 2026-11-28.
 */
const at = (file: string, line: number) => `${file}:${line}`;

/** The first pipe table under the heading that matches `title`. Throws on a malformed table. */
function tableUnder(md: string, title: RegExp, file: string): Table {
  // CRLF too: a Windows checkout with autocrlf must parse like the CI runner.
  const lines = md.split(/\r?\n/);
  const start = lines.findIndex((l) => title.test(l));
  if (start < 0) throw new Error(`${file}: no heading matches ${title}`);
  let i = start + 1;
  while (i < lines.length && !lines[i].startsWith("|")) {
    if (/^#{1,6} /.test(lines[i])) throw new Error(`${at(file, i + 1)}: heading ${title} has no table above the next heading`);
    i += 1;
  }
  const headerLine = i + 1;
  const header = cellsOf(lines[i] ?? "");
  i += 1;
  if (!/^\|[\s:|-]+\|?$/.test(lines[i] ?? "")) {
    throw new Error(`${at(file, i + 1)}: the table under ${title} has no |---| separator row after its header`);
  }
  i += 1;
  const rows: Row[] = [];
  for (; i < lines.length && lines[i].startsWith("|"); i += 1) {
    const cells = cellsOf(lines[i]);
    if (cells.length !== header.length) {
      throw new Error(
        `${at(file, i + 1)}: row "${cells[0]}" has ${cells.length} cells, expected ${header.length} ` +
          `(${header.join(" | ")}); a literal | inside a cell splits it`,
      );
    }
    rows.push({ line: i + 1, cells });
  }
  return { header, headerLine, rows };
}

function expectHeader(t: Table, want: string[], file: string, where: string): void {
  if (t.header.join("|") !== want.join("|")) {
    throw new Error(`${at(file, t.headerLine)}: ${where} header is [${t.header.join(", ")}], expected [${want.join(", ")}]`);
  }
}

/**
 * A Wave cell holds one wave id and nothing else. The roadmap page groups by
 * it, and `cargo xtask devlog roadmap` (ROADMAP.md) skips a row whose cell is
 * anything longer, so a note here would hide the entry there without a word.
 */
function asWave(value: string, where: string, hint = ""): Wave {
  if ((WAVES as readonly string[]).includes(value)) return value as Wave;
  throw new Error(`${where}: Wave cell is "${value}", expected exactly one of ${WAVES.join(", ")}${hint}`);
}

const WAVE_HINT =
  ' (the bare id, per "Cell format in sections A to F" in CATALOG.md; a status note goes in the "What, for whom" cell)';

function asId(value: string, where: string): string {
  // Lower-case, digits, hyphens and dots: "sdks-1.0" is a real id.
  if (/^[a-z0-9][a-z0-9.-]*$/.test(value)) return value;
  throw new Error(`${where}: id "${value}" is not a lower-case slug (a-z, 0-9, "." and "-", starting with a letter or digit)`);
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
  const seen = new Map<string, number>();
  // A to F are the wave-scheduled tables. G is a fact list and H has no wave
  // column; I is the horizon table, parsed separately.
  for (const { key, name } of sections.filter((s) => s.key <= "F")) {
    const t = tableUnder(md, new RegExp(`^## ${key}\\. `), file);
    expectHeader(t, ENTRY_HEADER, file, `section ${key}`);
    for (const { line, cells } of t.rows) {
      const [id, what, builds, gate, size, wave] = cells;
      const where = `${at(file, line)}: section ${key}, row ${id}`;
      asId(id, where);
      const first = seen.get(id);
      if (first !== undefined) throw new Error(`${where}: duplicate id, first used at line ${first}`);
      seen.set(id, line);
      if (size !== "S" && size !== "M" && size !== "L") {
        throw new Error(`${where}: Size cell is "${size}", expected exactly one of S, M, L`);
      }
      out.push({
        id,
        section: key,
        sectionName: name,
        what: plain(what),
        builds: plain(builds),
        gate: plain(gate),
        gateKind: gateKind(plain(gate)),
        size,
        wave: asWave(wave, where, WAVE_HINT),
      });
    }
  }
  return out;
}

function parseWaves(md: string, file: string): Ecosystem["waves"] {
  const t = tableUnder(md, /^## Waves$/, file);
  expectHeader(t, ["Wave", "When", "Rule"], file, "Waves");
  if (t.rows.length !== WAVES.length) {
    throw new Error(`${at(file, t.headerLine)}: Waves table has ${t.rows.length} rows, expected ${WAVES.length} (${WAVES.join(", ")})`);
  }
  return t.rows.map(({ line, cells: [w, when, rule] }) => ({
    id: asWave(w, `${at(file, line)}: Waves table`),
    when: plain(when),
    rule: plain(rule),
  }));
}

function parseHorizon(md: string, file: string): Horizon[] {
  const t = tableUnder(md, /^## I\. /, file);
  expectHeader(t, HORIZON_HEADER, file, "section I");
  return t.rows.map(({ line, cells: [id, need, rung, builds, prior] }) => {
    const where = `${at(file, line)}: section I, row ${id}`;
    if (!(RUNGS as readonly string[]).includes(rung)) {
      throw new Error(`${where}: Rung cell is "${rung}", expected exactly one of ${RUNGS.join(", ")}`);
    }
    const link = /\]\((\.\.\/prior-art\/[^)\s]+)\)/.exec(prior);
    return {
      id: asId(id, where),
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
  if (t.header[0] !== "Dimension" || t.header.length < 2) {
    throw new Error(`${at(file, t.headerLine)}: verdict matrix header is [${t.header.join(", ")}], expected [Dimension, <chain code>, ...]`);
  }
  const tally = Object.fromEntries(VERDICTS.map((v) => [v, 0])) as Record<Verdict, number>;
  const rows = t.rows.map(({ line, cells }) => {
    const verdicts = cells.slice(1).map((c, k) => {
      if (!(VERDICTS as readonly string[]).includes(c)) {
        throw new Error(
          `${at(file, line)}: verdict matrix, row "${cells[0]}", column ${t.header[k + 1]}: cell is "${c}", ` +
            `expected exactly one of ${VERDICTS.join(", ")}`,
        );
      }
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
  expectHeader(t, ["Trail cell", "Closed by", "Notes"], file, "section 4");
  return t.rows.map(({ cells: [cell, closedBy, notes] }) => ({
    cell: plain(cell),
    closedBy: plain(closedBy),
    ids: [...closedBy.matchAll(/`([a-z0-9-]+)`/g)].map((m) => m[1]).filter((id) => knownIds.has(id)),
    notes: plain(notes),
  }));
}

/** Repository-relative names, so an error is a path a reader can open. */
const CATALOG_FILE = "docs/ecosystem/CATALOG.md";
const COMPETITORS_FILE = "docs/ecosystem/COMPETITORS.md";

/** Everything the roadmap page shows, read from the two docs. */
export function loadEcosystem(): Ecosystem {
  return parseEcosystem(read("CATALOG.md"), read("COMPETITORS.md"));
}

/**
 * The same, from the two documents' text: the test suite feeds it a broken
 * row to prove the build stops with the file, line, row and expected form.
 */
export function parseEcosystem(catalog: string, competitors: string): Ecosystem {
  const catalogFile = CATALOG_FILE;
  const competitorsFile = COMPETITORS_FILE;

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
