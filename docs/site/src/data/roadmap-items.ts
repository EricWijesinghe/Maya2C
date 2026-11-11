// Roadmap items for /ecosystem/roadmap/. Edit this file to update the page.
// Rules (CLAUDE.md): "review" only with a real open PR in `pr`; "queued" means
// not started; months are targets, never promises; `others` is neutral and says
// what exists elsewhere, not that Maya2C is better. Checked against
// `gh pr list --state open` on 2026-11-12.

export type Status = "shipped" | "review" | "queued" | "design";
export type Category = "wallet" | "tokens" | "privacy" | "scaling" | "ai" | "interop";
export type LaneId = "m1" | "m2" | "m3" | "later";

export interface RoadmapItem {
  id: string;
  name: string;
  lane: LaneId;
  cat: Category;
  status: Status;
  what: string;
  others: string;
  pr?: number;
  postGenesis?: boolean;
}

export const REPO_PR = "https://github.com/EricWijesinghe/Maya2C/pull/";

export const LANES: { id: LaneId; title: string; sub: string }[] = [
  { id: "m1", title: "Nov to Dec", sub: "Target: finish what is in flight" },
  { id: "m2", title: "Dec to Jan", sub: "Target: scaling core and token standards" },
  { id: "m3", title: "Jan to Feb", sub: "Target: accounts, staking, bridges" },
  { id: "later", title: "Later waves", sub: "After launch; ordered, not dated" },
];

export const CATEGORIES: Record<Category, string> = {
  wallet: "Wallet",
  tokens: "Tokens",
  privacy: "Privacy",
  scaling: "Scaling",
  ai: "AI",
  interop: "Interop",
};

export const STATUS_LABEL: Record<Status, string> = {
  shipped: "Shipped",
  review: "In review",
  queued: "Queued",
  design: "Design",
};

export const roadmapItems: RoadmapItem[] = [
  // Nov to Dec
  { id: "finality", name: "DAG-BFT finality", lane: "m1", cat: "scaling", status: "shipped",
    what: "Blocks are finalized by validator certificates, so a confirmed payment is final.",
    others: "Tendermint-style BFT chains and Ethereum's finality gadget also give explicit finality." },
  { id: "dex", name: "On-chain swaps", lane: "m1", cat: "tokens", status: "shipped",
    what: "A built-in exchange module with a slippage guard on every swap.",
    others: "Uniswap-style AMMs on EVM chains and order books on Solana and Cosmos chains." },
  { id: "sdk-gateway", name: "Rust and Go SDK clients", lane: "m1", cat: "interop", status: "shipped",
    what: "A Rust gateway client and a Go address codec for developers.",
    others: "Most chains ship JS, Rust and Go SDKs." },
  { id: "clear-sign", name: "Clear signing wallet", lane: "m1", cat: "wallet", status: "review", pr: 446,
    what: "The wallet shows what you sign in plain words, plus an address book and payment requests.",
    others: "Ledger clear signing and EIP-712 typed data show readable prompts on EVM chains." },
  { id: "undo-send", name: "Undo-send and payee check", lane: "m1", cat: "wallet", status: "review", pr: 460,
    what: "A short hold before a payment goes out, and a check that the payee is who you expect.",
    others: "Some wallets add address warnings and transaction simulation." },
  { id: "checkout", name: "Merchant checkout", lane: "m1", cat: "wallet", status: "review", pr: 418,
    what: "Payment links, QR invoices and signed webhooks for shops.",
    others: "Solana Pay and BitPay-style checkouts do the same job." },
  { id: "chat-pay", name: "Pay inside chat", lane: "m1", cat: "wallet", status: "review", pr: 441,
    what: "Request and receive payments inside a chat; the wallet still signs.",
    others: "Telegram wallets and in-app payment requests exist on several chains." },
  { id: "addr-text", name: "Bech32m addresses", lane: "m1", cat: "wallet", status: "review", pr: 410,
    what: "Readable maya1 addresses in the pool login, explorer and faucet.",
    others: "Bitcoin segwit and Cosmos chains use Bech32 text addresses." },
  { id: "indexer", name: "Public indexer API", lane: "m1", cat: "interop", status: "review", pr: 414,
    what: "A documented read API for blocks, accounts and transfers.",
    others: "The Graph, Subsquid and chain-run explorer APIs." },
  { id: "light-finality", name: "Finality light client", lane: "m1", cat: "interop", status: "review", pr: 442,
    what: "A client that checks finality certificates without running a full node.",
    others: "Ethereum sync committees and IBC light clients check headers in a similar way." },
  { id: "finality-bench", name: "Finality latency bench", lane: "m1", cat: "scaling", status: "review", pr: 425,
    what: "A repeatable tool that measures submit-to-finalized time, with hardware and mix stated.",
    others: "Chains publish latency figures; methods and setups vary." },
  { id: "notary", name: "File notary", lane: "m1", cat: "interop", status: "review", pr: 449,
    what: "Timestamp a file by its hash and check it later with a receipt.",
    others: "OpenTimestamps anchors hashes in Bitcoin." },
  { id: "mcp", name: "AI assistant read access", lane: "m1", cat: "ai", status: "review", pr: 440,
    what: "An MCP server that lets an AI assistant read the chain and draft, not sign, transfers.",
    others: "Several chains now publish MCP or agent toolkits." },
  { id: "shielded-bit", name: "Shielded pool switch", lane: "m1", cat: "privacy", status: "review", pr: 420, postGenesis: true,
    what: "A schedule-activated switch for the shielded pool; it stays off until a written activation height.",
    others: "Zcash and Penumbra have shielded pools live." },
  { id: "account-authority", name: "Account guardians", lane: "m1", cat: "wallet", status: "review", pr: 447, postGenesis: true,
    what: "Recover an account through guardians instead of a paper seed; dark until activated.",
    others: "ERC-4337 wallets and Argent-style social recovery." },
  { id: "sentinel-v2", name: "Self-healing watchdog", lane: "m1", cat: "ai", status: "queued",
    what: "A watchdog that spots faults and proposes fixes; a person approves every action.",
    others: "Node-operator alerting stacks exist on most chains." },

  // Dec to Jan
  { id: "parallel", name: "Parallel execution", lane: "m2", cat: "scaling", status: "queued", postGenesis: true,
    what: "Run non-conflicting transactions side by side; the crate exists as research, not in consensus.",
    others: "Solana Sealevel, Aptos Block-STM and Sui process transactions in parallel." },
  { id: "stateless", name: "Stateless validation", lane: "m2", cat: "scaling", status: "queued", postGenesis: true,
    what: "Check blocks with witnesses instead of full state; built and dark today.",
    others: "Ethereum's Verkle and stateless research tracks." },
  { id: "tokens", name: "User-issued tokens", lane: "m2", cat: "tokens", status: "design", pr: 465, postGenesis: true,
    what: "Anyone can issue a fungible token under a common standard.",
    others: "ERC-20, SPL tokens and Cosmos bank denoms." },
  { id: "nft", name: "Collectibles (NFTs)", lane: "m2", cat: "tokens", status: "design", pr: 465, postGenesis: true,
    what: "A standard for unique items and collections.",
    others: "ERC-721 and ERC-1155, Metaplex on Solana." },
  { id: "names", name: "Name service", lane: "m2", cat: "wallet", status: "design", pr: 465, postGenesis: true,
    what: "Human-readable names that point to addresses.",
    others: "ENS and Solana Name Service." },
  { id: "paymaster", name: "Sponsored fees", lane: "m2", cat: "wallet", status: "design", pr: 465, postGenesis: true,
    what: "An app can pay the fee for its users.",
    others: "ERC-4337 paymasters and Sui sponsored transactions." },
  { id: "ledger", name: "Hardware wallet app", lane: "m2", cat: "wallet", status: "queued",
    what: "Sign with a Ledger device; the app exists in the tree and needs finishing and tests.",
    others: "Ledger apps exist for most major chains." },
  { id: "multisig", name: "Multisig wallet screen", lane: "m2", cat: "wallet", status: "queued",
    what: "A screen for shared accounts that need several approvals.",
    others: "Safe on EVM chains, Squads on Solana." },
  { id: "extension", name: "Browser extension wallet", lane: "m2", cat: "wallet", status: "queued",
    what: "A wallet inside the browser for web apps.",
    others: "MetaMask and Phantom." },

  // Jan to Feb
  { id: "aa", name: "Account abstraction", lane: "m3", cat: "wallet", status: "design", pr: 465, postGenesis: true,
    what: "Session keys and spending limits; smart-account code exists as research.",
    others: "ERC-4337, Starknet and Sui accounts." },
  { id: "liquid", name: "Liquid staking", lane: "m3", cat: "tokens", status: "design", pr: 465, postGenesis: true,
    what: "Stake and keep a tradable receipt token.",
    others: "Lido, Jito and Stride." },
  { id: "sched", name: "Scheduled payments", lane: "m3", cat: "wallet", status: "design", pr: 465, postGenesis: true,
    what: "Recurring payments with a limit you set.",
    others: "Gelato automation and Cosmos authz grants." },
  { id: "oracle", name: "Price feed", lane: "m3", cat: "interop", status: "queued",
    what: "A price feed built on the existing oracle crate.",
    others: "Chainlink and Pyth." },
  { id: "bridge", name: "Ethereum bridge", lane: "m3", cat: "interop", status: "design",
    what: "Move value to and from Ethereum with a light-client check; a research crate exists.",
    others: "IBC, LayerZero and Wormhole use different trust models." },
  { id: "stealth", name: "Stealth addresses", lane: "m3", cat: "privacy", status: "design", pr: 465, postGenesis: true,
    what: "One-time receiving addresses built on a post-quantum key exchange.",
    others: "Monero and Ethereum stealth-address proposals." },
  { id: "kani", name: "Machine-checked apply path", lane: "m3", cat: "scaling", status: "queued",
    what: "Proofs that block application keeps its invariants.",
    others: "Formal verification is used on some L1 cores and smart contracts." },
  { id: "ai-debug", name: "AI debug assistant", lane: "m3", cat: "ai", status: "queued",
    what: "An assistant that explains failed transactions; its answers carry evidence.",
    others: "Block explorers and Tenderly-style tools simulate and trace." },
  { id: "mobile", name: "Mobile wallet", lane: "m3", cat: "wallet", status: "queued",
    what: "An Android light-client wallet; iOS follows later.",
    others: "Most major chains have one." },
  { id: "evm", name: "EVM-compatible contracts", lane: "m3", cat: "interop", status: "queued",
    what: "Run EVM contracts; the multivm crate exists as research and its cost is unmeasured.",
    others: "Hundreds of chains run the EVM." },
  { id: "rollup", name: "Rollup settlement", lane: "m3", cat: "scaling", status: "queued",
    what: "Settle batches from layer-2 systems; a flash-payment channel crate exists.",
    others: "OP Stack and zk rollup stacks." },

  // Later
  { id: "chat-v2", name: "Group chat", lane: "later", cat: "interop", status: "design",
    what: "Group chats on MLS with attachments and multiple devices; the ADR is proposed.",
    others: "Signal, Matrix and Wire use group protocols." },
  { id: "flash", name: "Instant payment channels", lane: "later", cat: "scaling", status: "design",
    what: "Off-chain channels for small, frequent payments.",
    others: "Lightning on Bitcoin; state channels on Ethereum." },
  { id: "sentinel-ai", name: "Sentinel recovery network", lane: "later", cat: "ai", status: "design",
    what: "A recovery network where AI only advises and people decide.",
    others: "Chain halt response is usually manual coordination." },
  { id: "sharding", name: "Dynamic sharding", lane: "later", cat: "scaling", status: "design", postGenesis: true,
    what: "Split work across shards; a per-node map exists as research.",
    others: "NEAR Nightshade and Ethereum's rollup-centric roadmap." },
];
