// The apps around the chain, for the homepage ecosystem explorer. `status`
// says what is true today (built = in the tree with tests and a release or
// a live service; research = tested, not on a production path; planned = no
// code yet). A card with a `ledger` id takes its status from features.toml at
// build (W-21): class RESEARCH is research, planned or stub is planned, and an
// unknown id fails the build. Cards without one change only with evidence.
import health from "../../public/health.json";
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

const releases = "https://github.com/EricWijesinghe/Maya2C/releases";

const cards: EcoApp[] = [
  { id: "wallet", name: "Maya2C Wallet", icon: "👛", note: "CLI + desktop", status: "built", ledger: "tauri-2-0-desktop-wallet", what: "Holds your post-quantum keys and signs every transfer twice, with ML-DSA-65 and SLH-DSA.", now: "Read the wallet guide, then download the desktop wallet.", href: "/guides/wallet/" },
  { id: "chat", name: "Maya2C Chat", icon: "💬", note: "PQ messenger", status: "built", ledger: "maya-chat", what: "A messenger with post-quantum key exchange, delivery receipts and stickers. Research software: unaudited, no group chats, no metadata privacy yet.", now: "Try it from the releases page, knowing it is unaudited.", href: releases },
  { id: "hub", name: "Maya2C Hub", icon: "🧭", note: "all apps, one window", status: "built", ledger: "maya2c-hub", what: "One desktop app that opens the others and runs the miner where you can see it.", now: "Not published yet. Mine from the command line today.", href: "/guides/start-mining/" },
  { id: "explorer", name: "Explorer", icon: "🔭", note: "blocks · txs · accounts", status: "built", ledger: "leptos-block-explorer-and-dashboard", what: "Every block, transaction and account on the chain, searchable, updated live.", now: "Open it. It is live whenever the testnet runs.", href: "https://explorer.maya2c.dev" },
  { id: "gateway", name: "Gateway", icon: "🔌", note: "REST · GraphQL", status: "built", ledger: "axum-rest-graphql-gateway", what: "The public API that apps and SDKs use. Apps never link the node itself.", now: "Read the API reference.", href: "/guides/api/" },
  { id: "sdks", name: "SDKs", icon: "🧰", note: "TS · Python · Go · Rust", status: "built", ledger: "multi-language-sdks", what: "Libraries for building on Maya2C in four languages, tested against a live node.", now: "Browse the source on GitHub.", href: "https://github.com/EricWijesinghe/Maya2C" },
  { id: "identity", name: "Identity", icon: "🪪", note: "proof of personhood", status: "research", what: "One identity across every app, without handing your data to any of them. In research: tested, not switched on.", now: "Read the ecosystem reference.", href: "/reference/ecosystem/" },
  { id: "governance", name: "Governance", icon: "🏛️", note: "representative", status: "planned", what: "Holders choose representatives who vote on upgrades. Planned for after mainnet.", now: "Read the governance design.", href: "/reference/governance/" },
  { id: "ai", name: "AI chat", icon: "✨", note: "on-device", status: "planned", what: "An assistant that runs on your own device. Planned; never part of consensus.", now: "Read the confidential-AI design.", href: "/reference/confidential-ai/" },
];

const ledger = health.subsystems as { id?: string; status: string; class: string }[];
function ledgerEntry(owner: string, id: string) {
  const s = ledger.find((x) => x.id === id);
  if (!s) throw new Error(`ecosystem-apps: ${owner} names ${id}, which features.toml does not have`);
  return s;
}
function fromLedger(a: EcoApp): EcoApp {
  if (!a.ledger) return a;
  const s = ledgerEntry(a.id, a.ledger);
  const status: AppStatus = s.class === "RESEARCH" ? "research" : s.status === "planned" || s.status === "stub" ? "planned" : "built";
  return { ...a, status };
}
export const ecoApps: EcoApp[] = cards.map(fromLedger);

// --- /ecosystem/ hub ---------------------------------------------------------
// Every item names either a features.toml subsystem (its chip is the ledger's
// class and status, read at build) or an open pull request (its chip says "in
// progress" and links the PR). Nothing else may set a status by hand.
const REPO = "https://github.com/EricWijesinghe/Maya2C";
const tree = (p: string) => `${REPO}/tree/master/${p}`;
const pull = (n: number) => `${REPO}/pull/${n}`;

export const HUB_GROUPS = [
  { id: "apps", label: "Apps" },
  { id: "sdks", label: "SDKs" },
  { id: "tools", label: "Developer and operator tools" },
  { id: "ai", label: "MCP server and agent tools" },
  { id: "bridges", label: "Bridges and swaps" },
] as const;
export type HubGroup = (typeof HUB_GROUPS)[number]["id"];
export type ChipTone = "ok" | "info" | "warn";

interface HubSource {
  id: string;
  name: string;
  icon: string; // decorative
  group: HubGroup;
  what: string;
  path?: string; // repo path shown on the card (ledger items)
  ledger?: string; // features.toml subsystem id
  pr?: number; // open PR, for items not in the ledger yet
}
export interface HubItem extends HubSource {
  chip: string;
  tone: ChipTone;
  href: string;
}

const hubSources: HubSource[] = [
  { id: "wallet", name: "Maya2C Wallet", icon: "👛", group: "apps", what: "Desktop wallet, hybrid post-quantum signatures.", path: "apps/wallet-gui", ledger: "tauri-2-0-desktop-wallet" },
  { id: "cli-wallet", name: "Command-line wallet", icon: "⌨️", group: "apps", what: "Keys, transfers and staking from a terminal.", path: "bins/l1-wallet", ledger: "command-line-wallet" },
  { id: "ledger-app", name: "Ledger device app", icon: "🔐", group: "apps", what: "ML-DSA signing on a hardware wallet.", path: "apps/ledger-maya2c", ledger: "ledger-hardware-wallet-app" },
  { id: "chat", name: "Maya2C Chat", icon: "💬", group: "apps", what: "Post-quantum P2P messenger. Unaudited.", path: "apps/chat-gui", ledger: "maya-chat" },
  { id: "hub", name: "Maya2C Hub", icon: "🧭", group: "apps", what: "Desktop miner and dashboard for the other apps.", path: "apps/hub", ledger: "maya2c-hub" },
  { id: "explorer", name: "Explorer", icon: "🔭", group: "apps", what: "Blocks, transactions and accounts, live.", path: "apps/explorer", ledger: "leptos-block-explorer-and-dashboard" },
  { id: "faucet", name: "Testnet faucet", icon: "🚰", group: "apps", what: "Test coins for developers.", path: "apps/faucet", ledger: "testnet-faucet" },
  { id: "status", name: "Chain status", icon: "📈", group: "apps", what: "Uptime and chain health page.", path: "apps/status", ledger: "chain-status" },
  { id: "reference-apps", name: "Reference apps", icon: "🧪", group: "apps", what: "Payments, DEX front end, DAO, vault, NFT game.", path: "crates/reference-apps", ledger: "reference-apps" },
  { id: "sdks", name: "FFI, WASM and JS SDKs", icon: "🧰", group: "sdks", what: "One core, bound to C, the browser and Node.", path: "sdks/", ledger: "multi-language-sdks" },
  { id: "rust-sdk", name: "Rust gateway SDK", icon: "🦀", group: "sdks", what: "maya2c-sdk client for the gateway. Not published.", path: "sdks/rust", ledger: "rust-gateway-sdk" },
  { id: "cpp-sdk", name: "C++ SDK", icon: "➕", group: "sdks", what: "C++ wrapper over sdk-ffi.", pr: 498 },
  { id: "gateway", name: "Gateway", icon: "🔌", group: "tools", what: "REST and GraphQL API every app uses.", path: "crates/api-gateway", ledger: "axum-rest-graphql-gateway" },
  { id: "webhooks", name: "Gateway webhooks", icon: "📡", group: "tools", what: "WebSocket stream and signed webhooks.", pr: 481 },
  { id: "public-api", name: "Public indexer API v1", icon: "🗂️", group: "tools", what: "Indexer API for wallets and dashboards.", pr: 414 },
  { id: "dev-cli", name: "maya2c developer CLI", icon: "🛠️", group: "tools", what: "Dev chain, replay, fork mode, time-travel debugger.", path: "bins/maya2c-cli", ledger: "maya2c-dev-cli" },
  { id: "testnet-kit", name: "Testnet kit", icon: "🧳", group: "tools", what: "Local devnet, explorer, faucet and attack lab.", path: "xtask/src/kit.rs", ledger: "testnet-kit" },
  { id: "deposit-watcher", name: "Exchange deposit watcher", icon: "🏦", group: "tools", what: "Exactly-once deposit crediting for exchanges.", path: "crates/deposit-watcher", ledger: "exchange-deposit-watcher" },
  { id: "merchant", name: "Merchant checkout", icon: "🧾", group: "tools", what: "Invoices and signed webhooks for shops.", pr: 418 },
  { id: "pool", name: "Mining pool service", icon: "⛏️", group: "tools", what: "Stratum V2 pool for miners.", path: "bins/pool-service", ledger: "mining-pool-service" },
  { id: "light-client", name: "SPV light client", icon: "🪶", group: "tools", what: "Header-chain client for phones and services.", path: "crates/light-client", ledger: "spv-light-client" },
  { id: "signer", name: "Remote validator signer", icon: "✍️", group: "tools", what: "Validator signing with slashing protection.", path: "bins/maya2c-signer", ledger: "remote-validator-signer" },
  { id: "design-system", name: "Design system", icon: "🎨", group: "tools", what: "WCAG-checked tokens and wallet flow model.", path: "crates/design-system", ledger: "design-system" },
  { id: "mcp", name: "MCP server", icon: "🤖", group: "ai", what: "AI assistants read the chain and draft transfers; never sign.", path: "bins/maya2c-mcp", ledger: "mcp-server" },
  { id: "mcp-debug", name: "MCP debugging tools", icon: "🔎", group: "ai", what: "Read-only AI debugging tools in maya2c-mcp.", pr: 482 },
  { id: "agent-registry", name: "Agent registry", icon: "🪪", group: "ai", what: "Off-chain registry and payments for AI agents.", pr: 489 },
  { id: "htlc", name: "HTLC atomic swaps", icon: "🔁", group: "bridges", what: "Hash-locked swaps with other chains.", path: "crates/htlc-lattice", ledger: "hash-lock-htlc-atomic-swaps" },
  { id: "btc-bridge", name: "Bitcoin bridge", icon: "🌉", group: "bridges", what: "Lock/mint over the SPV header chain.", path: "crates/btc-bridge", ledger: "btc-lock-mint-bridge" },
  { id: "eth-light", name: "Ethereum light client", icon: "🔗", group: "bridges", what: "Beacon sync-committee verification.", path: "crates/interop", ledger: "eth-beacon-light-client" },
  { id: "iso20022", name: "ISO 20022 bridge", icon: "🏛️", group: "bridges", what: "Bank payment messages to L1 transfers.", path: "crates/iso20022", ledger: "iso-20022-l1-bridge" },
];

function toHub(s: HubSource): HubItem {
  if (s.ledger) {
    if (!s.path) throw new Error(`ecosystem-apps: ${s.id} is in the ledger but names no repo path`);
    const e = ledgerEntry(s.id, s.ledger);
    const tone: ChipTone = e.status === "verified" ? "ok" : e.status === "working" ? "info" : "warn";
    const chip = e.class === "REAL" ? e.status : `${e.class.toLowerCase()} · ${e.status}`;
    return { ...s, chip, tone, href: tree(s.path) };
  }
  if (!s.pr) throw new Error(`ecosystem-apps: ${s.id} has neither a ledger id nor a PR`);
  return { ...s, chip: "in progress", tone: "warn", href: pull(s.pr) };
}
export const hubItems: HubItem[] = hubSources.map(toHub);

// "Build on Maya2C": what exists, where the docs are, what is planned. Each
// `exists` and `planned` name points at a hub item, a ledger id or a catalog row.
export interface EntryPoint {
  id: string;
  icon: string;
  title: string;
  exists: string[]; // hub item ids
  docs: { label: string; href: string }[];
  planned: string; // catalog rows, by id
}
export const entryPoints: EntryPoint[] = [
  { id: "wallets", icon: "👛", title: "Wallets", exists: ["wallet", "cli-wallet", "ledger-app", "sdks"], docs: [{ label: "Wallet integration", href: "/guides/wallet/" }, { label: "Custody", href: `${REPO}/blob/master/docs/integrations/CUSTODY.md` }], planned: "wallet-v2 (PR #446), wallet-mobile, wallet-extension" },
  { id: "exchanges", icon: "🏦", title: "Exchanges", exists: ["deposit-watcher", "gateway", "light-client"], docs: [{ label: "Exchange integration", href: `${REPO}/blob/master/docs/integrations/EXCHANGES.md` }, { label: "API reference", href: "/guides/api/" }], planned: "reserves-proof, exchange-ui" },
  { id: "explorers", icon: "🔭", title: "Explorers and indexers", exists: ["explorer", "gateway"], docs: [{ label: "API reference", href: "/guides/api/" }], planned: "public-api (PR #414), explorer-v2" },
  { id: "bridges", icon: "🔁", title: "Bridges", exists: ["htlc", "btc-bridge", "eth-light"], docs: [{ label: "Cross-chain design", href: "/reference/crosschain/" }], planned: "btc-bridge-app (needs a security review and a bridge ADR)" },
  { id: "sdks", icon: "🧰", title: "SDKs", exists: ["sdks", "rust-sdk", "dev-cli"], docs: [{ label: "SDK reference", href: "/reference/sdk/" }, { label: "Compiling a contract", href: "/guides/contracts/" }], planned: "sdks-1.0 (versioned releases, needs publishing approval), C++ SDK (PR #498)" },
  { id: "business", icon: "🧾", title: "Business integrations", exists: ["iso20022", "mcp"], docs: [{ label: "ISO 20022", href: "/reference/iso20022/" }, { label: "MCP server", href: "/reference/mcp-server/" }], planned: "merchant-checkout (PR #418), gateway webhooks (PR #481), pos-terminal" },
];

// Partners appear here only once a partner has agreed in writing to be listed,
// with that consent on file. None has: the list is empty, and the page says so.
export interface Partner {
  name: string;
  what: string;
  href: string;
}
export const partners: Partner[] = [];
