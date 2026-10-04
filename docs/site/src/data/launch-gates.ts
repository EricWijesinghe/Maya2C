// LAUNCH.md's gates as `cargo xtask go-no-go` computed them on 2026-09-28
// (8 PASS, 5 FAIL, 3 NEEDS HUMAN). A gate only turns green when the command
// does; copy its output here, never an intention.
export type GateState = "done" | "next" | "later";

export const gates: { name: string; detail: string; state: GateState; href: string }[] = [
  { name: "Localnet", detail: "4 validators over real libp2p: agree, survive a kill, catch up", state: "done", href: "/guides/quickstart" },
  { name: "Devnet software", detail: "DAG-BFT finality, staking, slashing, fee market, SLO metrics", state: "done", href: "/reference/blockgraph" },
  { name: "Operations rehearsed", detail: "state sync, crash restore, coordinated restart", state: "done", href: "/guides/validators" },
  { name: "Public testnet", detail: "maya-testnet-1 is live: rpc, faucet and chat relay public; one validator for now", state: "done", href: "/guides/testnet" },
  { name: "Independent validators", detail: "four operators on separate machines, so one failure cannot halt the chain", state: "next", href: "/guides/validators" },
  { name: "Attacknet + incentivised testnet", detail: "4 weeks each, from the day the testnet is live", state: "later", href: "/reference/empire" },
  { name: "External audit", detail: "no audit has reported yet", state: "later", href: "/reference/architecture-vision" },
  { name: "Mainnet", detail: "blocked until every gate above passes", state: "later", href: "/reference/empire" },
];
