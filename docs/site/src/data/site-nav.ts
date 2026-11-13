// The site's main navigation, shared by the header bar, its menu (splash
// pages) and the docs sidebar's phone menu, so the three never drift.
// One entry per audience (try it, build on it, run a node) plus the live
// services. Testnet and Validators sit under "Run a node" since 2026-11-10.
import { testnet } from "./network-state";

export type NavItem = { label: string; href: string; match?: string[]; external?: boolean };

export const NAV: NavItem[] = [
  { label: "Tour", href: "/experience/", match: ["/experience"] },
  // The API reference is the build landing; "Get started" owns the quickstart.
  { label: "Build", href: "/guides/api/", match: ["/guides/api", "/guides/wallet", "/guides/contracts", "/guides/passkey-unlock"] },
  { label: "Run a node", href: "/guides/help-run/", match: ["/guides/help-run", "/guides/testnet", "/guides/validators", "/guides/mining", "/guides/start-mining"] },
  // While the testnet is paused the explorer and status hosts are down (HTTP
  // 530): point at the site's own pages instead of a Cloudflare error (W-03).
  // "Health", not "Status": beside "Stats" the two read as one word.
  ...(testnet.paused
    ? [{ label: "Stats", href: "/stats/", match: ["/stats"] }, { label: "Health", href: "/health/", match: ["/health"] }]
    : [{ label: "Explorer", href: "https://explorer.maya2c.dev/", external: true }, { label: "Status", href: "https://status.maya2c.dev/", external: true }]),
];

// One call to action, the developer's first step. The author credit lives in the footer.
export const START = { label: "Get started", href: "/guides/quickstart/" };

export const REPO = "https://github.com/EricWijesinghe/Maya2C";

export const isCurrent = (item: NavItem, path: string) => item.match?.some((m) => path.startsWith(m)) ?? false;
