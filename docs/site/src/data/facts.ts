// The homepage facts band: every figure is read from a file the build writes
// (public/health.json from features.toml, public/progress.json from
// PROGRESS.md, the mainnet gates and GitHub), never typed (Standing Orders 1-2).
// A figure the build could not get says "unknown" instead of a guess.
const repo = "https://github.com/EricWijesinghe/Maya2C";

export interface Health {
  ledger: { total: number; by_status: Record<string, number> };
  tests: { passed: number; failed: number; as_of: string };
}
export interface Progress {
  generated: string;
  gates: { met: number; total: number };
  latest_release: { tag: string; at: string } | null;
}
export interface Fact { value: string; label: string; source: string; href: string }

const day = (iso: string) => iso.slice(0, 10);

export function homeFacts(h: Health, p: Progress): Fact[] {
  return [
    {
      value: `${h.ledger.by_status.verified ?? 0} / ${h.ledger.total}`,
      label: "subsystems verified",
      source: "features.toml, each named test checked in CI",
      href: `${repo}/blob/master/features.toml`,
    },
    {
      value: `${p.gates.met} / ${p.gates.total}`,
      label: "mainnet gates met",
      source: `mainnet plan, counted ${day(p.generated)}`,
      href: `${repo}/blob/master/docs/mainnet-v1-plan.md`,
    },
    {
      value: h.tests.passed.toLocaleString("en-US"),
      label: h.tests.failed === 0 ? "tests passing, none failing" : `tests passing, ${h.tests.failed} failing`,
      source: `last full count ${h.tests.as_of}`,
      href: `${repo}/actions`,
    },
    {
      value: p.latest_release?.tag.replace(/^node-/, "") ?? "unknown",
      label: "newest node release",
      source: p.latest_release ? `published ${day(p.latest_release.at)}` : "GitHub unreachable at build",
      href: `${repo}/releases`,
    },
  ];
}
