// LAUNCH.md's gates as `cargo xtask go-no-go` computed them on 2026-09-28
// (8 PASS, 5 FAIL, 3 NEEDS HUMAN). A gate only turns green when the command
// does; copy its output here, never an intention.
// Gates 5-7 restated for a one-operator project on 2026-10-05 (ADR-042).
export type GateState = "done" | "next" | "later";

export const gates: { name: string; detail: string; state: GateState; href: string }[] = [
  { name: "Localnet", detail: "4 validators over real libp2p: agree, survive a kill, catch up", state: "done", href: "/guides/quickstart" },
  { name: "Devnet software", detail: "DAG-BFT finality, staking, slashing, fee market, SLO metrics", state: "done", href: "/reference/blockgraph" },
  { name: "Operations rehearsed", detail: "state sync, crash restore, coordinated restart", state: "done", href: "/guides/validators" },
  { name: "Public testnet", detail: "maya-testnet-1 is live: rpc, faucet and chat relay public; one validator for now", state: "done", href: "/guides/testnet" },
  { name: "Foundation validator set", detail: "four project-run validators on four machines in different regions, so one failure cannot halt the chain; outside operators join after launch (ADR-042)", state: "next", href: "/guides/validators" },
  { name: "Attacknet, 4 weeks", detail: "self-run attacknet daily (crashes, stolen key, floods, outages) plus a public break-it programme for credit; paid incentives after funding (ADR-042)", state: "later", href: "/reference/empire" },
  { name: "Pre-audit + audit applied for", detail: "pre-audit package complete and free audit programmes applied to; mainnet carries an \"external audit pending\" label until a report lands (ADR-042)", state: "later", href: "/reference/architecture-vision" },
  { name: "Mainnet", detail: "blocked until every gate above passes", state: "later", href: "/reference/empire" },
];
