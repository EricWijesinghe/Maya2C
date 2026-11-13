// The apps around the chain, for the homepage ecosystem explorer. `status`
// says what is true today (built = in the tree with tests and a release or
// a live service; research = tested, not on a production path; planned = no
// code yet). Change a status only with the evidence it names.
export type AppStatus = "built" | "research" | "planned";

export interface EcoApp {
  id: string;
  name: string;
  icon: string; // one emoji, decorative (aria-hidden)
  note: string;
  status: AppStatus;
  what: string;
  now: string; // what a visitor can do with it today
  href: string;
}

const releases = "https://github.com/EricWijesinghe/Maya2C/releases";

export const ecoApps: EcoApp[] = [
  { id: "wallet", name: "Wallet", icon: "👛", note: "CLI + desktop", status: "built", what: "Holds your post-quantum keys and signs every transfer twice, with ML-DSA-65 and SLH-DSA.", now: "Read the wallet guide, then download the desktop wallet.", href: "/guides/wallet/" },
  { id: "chat", name: "Maya Chat", icon: "💬", note: "PQ messenger", status: "built", what: "A messenger with post-quantum key exchange, delivery receipts and stickers.", now: "Download it from the releases page.", href: releases },
  { id: "hub", name: "Hub", icon: "🧭", note: "all apps, one window", status: "built", what: "One desktop app that opens the others and runs the miner where you can see it.", now: "Download it from the releases page.", href: releases },
  { id: "explorer", name: "Explorer", icon: "🔭", note: "blocks · txs · accounts", status: "built", what: "Every block, transaction and account on the chain, searchable, updated live.", now: "Open it. It is live whenever the testnet runs.", href: "https://explorer.maya2c.dev" },
  { id: "gateway", name: "Gateway", icon: "🔌", note: "REST · GraphQL", status: "built", what: "The public API that apps and SDKs use. Apps never link the node itself.", now: "Read the API reference.", href: "/guides/api/" },
  { id: "sdks", name: "SDKs", icon: "🧰", note: "TS · Python · Go · Rust", status: "built", what: "Libraries for building on Maya2C in four languages, tested against a live node.", now: "Browse the source on GitHub.", href: "https://github.com/EricWijesinghe/Maya2C" },
  { id: "identity", name: "Identity", icon: "🪪", note: "proof of personhood", status: "research", what: "One identity across every app, without handing your data to any of them. In research: tested, not switched on.", now: "Read the ecosystem reference.", href: "/reference/ecosystem/" },
  { id: "governance", name: "Governance", icon: "🏛️", note: "representative", status: "planned", what: "Holders choose representatives who vote on upgrades. Planned for after mainnet.", now: "Read the governance design.", href: "/reference/governance/" },
  { id: "ai", name: "AI chat", icon: "✨", note: "on-device", status: "planned", what: "An assistant that runs on your own device. Planned; never part of consensus.", now: "Read the confidential-AI design.", href: "/reference/confidential-ai/" },
];
