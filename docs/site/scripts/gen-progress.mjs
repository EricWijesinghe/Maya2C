// Emits public/progress.json, the site's self-updating progress band, from
// sources that change without anyone editing the site: the checkboxes in
// PROGRESS.md and BACKLOG.md, the mainnet gates, and GitHub (master CI, PRs,
// the newest release). Every figure is counted, never typed (Standing Order 1).
// GitHub unreachable: those fields are null and the build goes on.
import { readFileSync, readdirSync, writeFileSync } from "node:fs";

const root = new URL("../../../", import.meta.url);
const read = (p) => { try { return readFileSync(new URL(p, root), "utf8"); } catch { return ""; } };
const boxes = (text) => ({
  done: (text.match(/^\s*- \[x\]/gim) || []).length,
  open: (text.match(/^\s*- \[ \]/gm) || []).length,
});
const gates = read("docs/site/src/data/mainnet-gates.ts");
const repo = "EricWijesinghe/Maya2C";
const headers = { "User-Agent": "maya2c-site", Accept: "application/vnd.github+json" };
if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
const gh = async (path) => {
  const url = path.startsWith("search/") ? `https://api.github.com/${path}` : `https://api.github.com/repos/${repo}/${path}`;
  try { const r = await fetch(url, { headers }); return r.ok ? r.json() : null; }
  catch { return null; }
};
const week = new Date(Date.now() - 7 * 864e5).toISOString().slice(0, 10);
const month = new Date(Date.now() - 30 * 864e5).toISOString();
const [ci, merged, open, release, recent, ciRuns, commits, mergedPrs] = await Promise.all([
  gh("actions/workflows/ci.yml/runs?branch=master&per_page=1&status=completed"),
  gh(`search/issues?q=repo:${repo}+is:pr+is:merged+merged:>=${week}&per_page=1`),
  gh("pulls?state=open&per_page=100"),
  gh("releases?per_page=1"),
  gh(`search/issues?q=repo:${repo}+is:pr+is:merged+-label:dependencies&sort=updated&order=desc&per_page=8`),
  gh("actions/workflows/ci.yml/runs?branch=master&per_page=50&status=completed"),
  gh(`commits?sha=master&since=${month}&per_page=100`),
  gh(`search/issues?q=repo:${repo}+is:pr+is:merged+-label:dependencies&sort=created&order=desc&per_page=50`),
]);
// Median hours from a PR opening to its merge, over the newest 50 merged PRs.
const median = (xs) => { if (!xs.length) return null; const s = [...xs].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };
const mergeHours = (mergedPrs?.items ?? []).map((i) => (Date.parse(i.closed_at) - Date.parse(i.created_at)) / 36e5).filter((h) => h >= 0);
const runs = (ciRuns?.workflow_runs ?? []).filter((r) => r.conclusion !== "cancelled" && r.conclusion !== "skipped");
// The newest finality bench report committed under reports/bench (cargo xtask bench-finality).
const finality = (() => {
  try {
    const f = readdirSync(new URL("reports/bench/", root)).filter((n) => /^finality-.*\.json$/.test(n)).sort().pop();
    if (!f) return null;
    const b = JSON.parse(read(`reports/bench/${f}`));
    return { report: `reports/bench/${f.replace(/\.json$/, ".md")}`, commit: b.commit, validators: b.validators, finalized: b.finalized, attempted: b.attempted, p50_ms: Math.round(b.latency_ms.p50), p90_ms: Math.round(b.latency_ms.p90), p99_ms: Math.round(b.latency_ms.p99), machine: b.machine, stamp: b.stamp };
  } catch { return null; }
})();
const run = ci?.workflow_runs?.[0];
const progress = {
  generated: new Date().toISOString(),
  gates: { met: (gates.match(/state: "met"/g) || []).length, total: (gates.match(/\{ n: \d+/g) || []).length },
  progress: boxes(read("PROGRESS.md")),
  // BACKLOG mixes plain bullets and checkboxes: every top-level item counts.
  backlog: { items: (read("BACKLOG.md").match(/^- /gm) || []).length, done: boxes(read("BACKLOG.md")).done },
  master_ci: run ? { conclusion: run.conclusion, at: run.updated_at, url: run.html_url } : null,
  merged_last_7_days: merged?.total_count ?? null,
  open_prs: Array.isArray(open) ? open.length : null,
  // What's new: the newest merged changes, by their own titles.
  recent: (recent?.items ?? []).map((i) => ({ title: i.title, url: i.html_url, at: i.closed_at })),
  // Of the newest 50 finished master CI runs (cancelled and skipped left out).
  ci_pass_rate: runs.length ? { passed: runs.filter((r) => r.conclusion === "success").length, of: runs.length } : null,
  // 100 is the page cap: the page shows "100+" at the cap rather than a guess.
  commits_last_30_days: Array.isArray(commits) ? commits.length : null,
  median_hours_to_merge: mergeHours.length ? Math.round(median(mergeHours) * 10) / 10 : null,
  finality,
  latest_release: release?.[0] ? { tag: release[0].tag_name, at: release[0].published_at } : null,
};
writeFileSync(new URL("../public/progress.json", import.meta.url), JSON.stringify(progress) + "\n");
console.log("progress.json:", JSON.stringify(progress));
