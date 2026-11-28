// Run: node --test src/data/ecosystem.test.ts (from docs/site)
//
// The roadmap page is parsed from docs/ecosystem/CATALOG.md at build time, and
// a malformed row stops the whole site build. From 2026-11-15 (#418) one Wave
// cell carried a status note ("W1. **Built:** ...") and no site deploy
// succeeded until it was found; the error named neither the path nor the line.
// These tests parse the real documents, then break one cell at a time and pin
// the error a writer sees: path, line, section, row, and the form expected.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { loadEcosystem, parseEcosystem, WAVES } from "./ecosystem.ts";

const doc = (name: string) => readFileSync(new URL(`../../../ecosystem/${name}`, import.meta.url), "utf8");
const catalog = doc("CATALOG.md");
const competitors = doc("COMPETITORS.md");

/** 1-based line of the table row whose first cell is `id`. */
function lineOf(md: string, id: string): number {
  const i = md.split(/\r?\n/).findIndex((l) => l.startsWith(`| ${id} |`));
  assert.ok(i >= 0, `row ${id} not found`);
  return i + 1;
}

/** The catalog with one row's text replaced, so each test breaks exactly one cell. */
function withRow(id: string, edit: (row: string) => string): string {
  const lines = catalog.split(/\r?\n/);
  const i = lineOf(catalog, id) - 1;
  const changed = edit(lines[i]);
  assert.notEqual(changed, lines[i], "the edit changed nothing");
  lines[i] = changed;
  return lines.join("\n");
}

const parseCatalog = (md: string) => () => parseEcosystem(md, competitors);

test("the real catalog and competitors pages parse", () => {
  const eco = loadEcosystem();
  assert.ok(eco.entries.length > 0, "no entries parsed");
  assert.deepEqual(eco.waves.map((w) => w.id), [...WAVES]);
  for (const e of eco.entries) assert.ok((WAVES as readonly string[]).includes(e.wave), `${e.id}: ${e.wave}`);
  const checkout = eco.entries.find((e) => e.id === "merchant-checkout");
  assert.equal(checkout?.wave, "W1");
  assert.equal(checkout?.section, "B");
});

test("a status note in the Wave cell names the file, line, row and expected form", () => {
  const broken = withRow("merchant-checkout", (r) => r.replace(/\| W1 \|$/, "| W1. **Built:** `apps/checkout` |"));
  const line = lineOf(broken, "merchant-checkout");
  assert.throws(parseCatalog(broken), (err: Error) => {
    assert.ok(
      err.message.startsWith(`docs/ecosystem/CATALOG.md:${line}: section B, row merchant-checkout: Wave cell is "W1. **Built:**`),
      err.message,
    );
    assert.match(err.message, /expected exactly one of W0, W1, W2, W3, W4/);
    assert.match(err.message, /status note goes in the "What, for whom" cell/);
    return true;
  });
});

test("a Size cell that is not S, M or L is refused with its line", () => {
  const broken = withRow("escrow", (r) => r.replace(/\| M \| W2 \|$/, "| medium | W2 |"));
  assert.throws(parseCatalog(broken), {
    message: `docs/ecosystem/CATALOG.md:${lineOf(broken, "escrow")}: section B, row escrow: Size cell is "medium", expected exactly one of S, M, L`,
  });
});

test("an extra | inside a cell is reported as a cell count, with its line", () => {
  const broken = withRow("escrow", (r) => r.replace("Two-party escrow", "Two-party | escrow"));
  assert.throws(parseCatalog(broken), (err: Error) => {
    assert.ok(err.message.startsWith(`docs/ecosystem/CATALOG.md:${lineOf(broken, "escrow")}: row "escrow" has 7 cells, expected 6`), err.message);
    return true;
  });
});

test("a duplicate id names both lines", () => {
  const broken = withRow("escrow", (r) => r.replace("| escrow |", "| merchant-checkout |"));
  const lines = broken.split(/\r?\n/);
  const [first, second] = lines.flatMap((l, i) => (l.startsWith("| merchant-checkout |") ? [i + 1] : []));
  assert.throws(parseCatalog(broken), {
    message: `docs/ecosystem/CATALOG.md:${second}: section B, row merchant-checkout: duplicate id, first used at line ${first}`,
  });
});

test("a CRLF checkout parses to the same page as LF", () => {
  const lf = parseEcosystem(catalog.replace(/\r\n/g, "\n"), competitors.replace(/\r\n/g, "\n"));
  const crlf = parseEcosystem(catalog.replace(/\r?\n/g, "\r\n"), competitors.replace(/\r?\n/g, "\r\n"));
  assert.deepEqual(crlf, lf);
});
