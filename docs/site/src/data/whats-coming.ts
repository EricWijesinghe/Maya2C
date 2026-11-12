// In-progress work, as it stands. Every row has an open pull request; none is
// shipped, and the page says "in progress" for each. Checked against
// `gh pr list --state open` on 2026-11-12. When a PR merges, the row moves to
// features.toml (and off this list) in the same change; when it closes, it goes.
export type Stage = "draft" | "review" | "proposed";

export interface Coming { name: string; line: string; stage: Stage; prs: number[]; postGenesis?: boolean }

export const asOf = "2026-11-12";

export const coming: Coming[] = [
  { name: "Tokens with a mint policy", line: "Create a token with an optional mint authority and a fixed 256-bit cap.", stage: "draft", prs: [487], postGenesis: true },
  { name: "NFTs and names", line: "Non-fungible assets and human-readable names on the chain.", stage: "draft", prs: [493], postGenesis: true },
  { name: "Sponsored fees", line: "An app can pay a user's fee, with a ceiling and an expiry the user signs.", stage: "draft", prs: [501], postGenesis: true },
  { name: "Scheduled payments", line: "Pay at a future height from a prepaid escrow.", stage: "draft", prs: [502], postGenesis: true },
  { name: "Hardware intelligence", line: "Opt-in GPU tuning in the Hub, node self-tuning off by default, advisory GPU verification.", stage: "review", prs: [492, 491, 494, 499, 497] },
  { name: "Agent registry", line: "An off-chain registry where AI agents publish what they do and get paid.", stage: "draft", prs: [489] },
  { name: "Chat mini-apps", line: "Small apps and wallet bubbles inside Maya2C Chat.", stage: "draft", prs: [495] },
  { name: "Intent solver market", line: "Say what you want; solvers compete to fill it, settled by hash-locked swaps.", stage: "draft", prs: [496] },
  { name: "C++ SDK", line: "Build on Maya2C from C++ through the existing FFI layer.", stage: "draft", prs: [498] },
];
