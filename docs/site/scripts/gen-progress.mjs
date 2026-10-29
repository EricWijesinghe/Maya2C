// Emits public/progress.json, the site's self-updating progress band, from
// sources that change without anyone editing the site: the checkboxes in
// PROGRESS.md and BACKLOG.md, the mainnet gates, and GitHub (master CI, PRs,
// the newest release). Every figure is counted, never typed (Standing Order 1).
// GitHub unreachable: those fields are null and the build goes on.
import { readFileSync, writeFileSync } from "node:fs";

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
const [ci, merged, open, release, recent] = await Promise.all([
  gh("actions/workflows/ci.yml/runs?branch=master&per_page=1&status=completed"),
  gh(`search/issues?q=repo:${repo}+is:pr+is:merged+merged:>=${week}&per_page=1`),
  gh("pulls?state=open&per_page=100"),
  gh("releases?per_page=1"),
  gh(`search/issues?q=repo:${repo}+is:pr+is:merged+-label:dependencies&sort=updated&order=desc&per_page=8`),
]);
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
  latest_release: release?.[0] ? { tag: release[0].tag_name, at: release[0].published_at } : null,
};
writeFileSync(new URL("../public/progress.json", import.meta.url), JSON.stringify(progress) + "\n");
console.log("progress.json:", JSON.stringify(progress));
