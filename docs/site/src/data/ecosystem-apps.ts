// The apps around the chain, for the homepage ecosystem explorer. `status`
// says what is true today (built = in the tree with tests and a release or
// a live service; research = tested, not on a production path; planned = no
// code yet). A card with a `ledger` id takes its status from features.toml at
// build (W-21): class RESEARCH is research, planned or stub is planned, and an
// unknown id fails the build. Cards without one change only with evidence.
import health from "../../public/health.json";
import { testnet } from "./network-state.ts";
import { releaseHref, release } from "./release.ts";
export type AppStatus = "built" | "research" | "planned";

export interface EcoApp {
  id: string;
  name: string;
  icon: string; // one emoji, decorative (aria-hidden)
  note: string;
  status: AppStatus;
  what: string;
  now: string; // what a visitor can do with it today
  ledger?: string; // features.toml subsystem id
  href: string;
}

const repo = "https://github.com/EricWijesinghe/Maya2C";

const cards: EcoApp[] = [
  { id: "wallet", name: "Maya2C Wallet", icon: "👛", note: "CLI + desktop", status: "built", ledger: "tauri-2-0-desktop-wallet", what: "Holds your post-quantum keys and signs every transfer twice, with ML-DSA-65 and SLH-DSA.", now: release.published ? "Read the wallet guide, then download the desktop wallet." : "Build from source today; the installer is not released yet.", href: "/guides/wallet/" },
  { id: "chat", name: "Maya2C Chat", icon: "💬", note: "PQ messenger", status: "built", ledger: "maya-chat", what: "A messenger with post-quantum key exchange, delivery receipts and stickers. Research software: unaudited, no group chats, no metadata privacy yet.", now: `Unaudited: read the ${release.tag} notes before you try it.`, href: releaseHref },
  { id: "hub", name: "Maya2C Hub", icon: "🧭", note: "all apps, one window", status: "built", ledger: "maya2c-hub", what: "One desktop app that opens the others and runs the miner where you can see it.", now: "Build from source (cargo xtask hub-bundle); no installer is published.", href: `${repo}/tree/master/apps/hub` },
  { id: "explorer", name: "Explorer", icon: "🔭", note: "blocks · txs · accounts", status: "built", ledger: "leptos-block-explorer-and-dashboard", what: "Every block, transaction and account on the chain, searchable, updated live.", ...(testnet.paused
    ? { now: "Offline until next genesis. The committed page baselines show every view.", href: `${repo}/tree/master/design/visual/baseline/explorer` }
    : { now: "Open it. It is live whenever the testnet runs.", href: "https://explorer.maya2c.dev" }) },
  { id: "gateway", name: "Gateway", icon: "🔌", note: "REST · GraphQL", status: "built", ledger: "axum-rest-graphql-gateway", what: "The public API that apps and SDKs use. Apps never link the node itself.", now: "Read the API reference.", href: "/guides/api/" },
  { id: "sdks", name: "SDKs", icon: "🧰", note: "TS · Python · Go · Rust", status: "built", ledger: "multi-language-sdks", what: "Libraries for building on Maya2C in four languages, tested against a live node.", now: "Browse the source on GitHub.", href: repo },
  { id: "identity", name: "Identity", icon: "🪪", note: "proof of personhood", status: "research", what: "One identity across every app, without handing your data to any of them. In research: tested, not switched on.", now: "Read the ecosystem reference.", href: "/reference/ecosystem/" },
  { id: "governance", name: "Governance", icon: "🏛️", note: "representative", status: "planned", what: "Holders choose representatives who vote on upgrades. Planned for after mainnet.", now: "Read the governance design.", href: "/reference/governance/" },
  { id: "ai", name: "AI chat", icon: "✨", note: "on-device", status: "planned", what: "An assistant that runs on your own device. Planned; never part of consensus.", now: "Read the confidential-AI design.", href: "/reference/confidential-ai/" },
];

const ledger = health.subsystems as { id?: string; status: string; class: string }[];
function fromLedger(a: EcoApp): EcoApp {
  if (!a.ledger) return a;
  const s = ledger.find((x) => x.id === a.ledger);
  if (!s) throw new Error(`ecosystem-apps: ${a.id} names ${a.ledger}, which features.toml does not have`);
  const status: AppStatus = s.class === "RESEARCH" ? "research" : s.status === "planned" || s.status === "stub" ? "planned" : "built";
  return { ...a, status };
}
export const ecoApps: EcoApp[] = cards.map(fromLedger);
