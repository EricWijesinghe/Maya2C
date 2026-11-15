// Run: node --test src/scripts/escape.test.ts (from docs/site)
import { test } from "node:test";
import assert from "node:assert/strict";
import { esc, safeHref } from "./escape.ts";

test("esc neutralises markup in every position", () => {
  assert.equal(esc('<img src=x onerror="a">'), "&lt;img src=x onerror=&quot;a&quot;&gt;");
  assert.equal(esc("it's & </b>"), "it&#39;s &amp; &lt;/b&gt;");
  assert.equal(esc(12), "12");
});

test("safeHref allows http(s) and same-site paths only", () => {
  assert.equal(safeHref("https://github.com/x"), "https://github.com/x");
  assert.equal(safeHref("/health/"), "/health/");
  assert.equal(safeHref("javascript:alert(1)"), "#");
  assert.equal(safeHref("//evil.example/x"), "#");
  assert.equal(safeHref('https://a.example/"onmouseover="x'), "https://a.example/&quot;onmouseover=&quot;x");
});
