// The ten mainnet v1 gates, mirrored from docs/mainnet-v1-plan.md. That file
// is the authority: a gate turns "met" here only after its row there says so
// and names the evidence. Never set a state from an intention.
export type MainnetGateState = "met" | "running" | "waiting";

export interface MainnetGate {
  n: number;
  name: string;
  line: string;
  state: MainnetGateState;
  evidence: string;
  href: string;
}

const repo = "https://github.com/EricWijesinghe/Maya2C/blob/master";

export const asOf = "2026-10-11";

export const mainnetGates: MainnetGate[] = [
  { n: 1, name: "Replay protection", line: "A signature commits to its chain: a testnet transfer cannot replay on mainnet", state: "met", evidence: "ADR-036", href: `${repo}/docs/adr/ADR-036-chain-bound-signatures.md` },
  { n: 2, name: "Shielded pool off", line: "Mainnet starts without shielded transfers; the guard refuses a genesis that enables them", state: "met", evidence: "ADR-037", href: `${repo}/docs/adr/ADR-037-mainnet-shielded-off.md` },
  { n: 3, name: "7 days, no halt", line: "The public testnet must run a week without stopping; the clock restarted 2026-10-10 19:25 KST", state: "running", evidence: "status page", href: "https://status.maya2c.dev/" },
  { n: 4, name: "Separate validators", line: "Four validators on separate machines, run by separate people", state: "waiting", evidence: "needs machines and operators", href: "/guides/validators/" },
  { n: 5, name: "Genesis ceremony", line: "Rehearsed and reproduced byte-identical; the real one needs gate 4", state: "waiting", evidence: "ceremony runbook", href: `${repo}/docs/runbooks/genesis-ceremony.md` },
  { n: 6, name: "Economics", line: "Fees only at launch: no emission, the base fee is burned", state: "met", evidence: "ADR-029", href: "/reference/fee-market/" },
  { n: 7, name: "CI and release", line: "Every check green, gate 1 and 2 reviews clean, binaries built from a tag", state: "running", evidence: "CI", href: "https://github.com/EricWijesinghe/Maya2C/actions" },
  { n: 8, name: "Validator rejoins", line: "A validator down for any length of time comes back on its own", state: "met", evidence: "attacknet attack 7", href: `${repo}/reports/attacknet/2026-10-10-6.md` },
  { n: 9, name: "Logs stay bounded", line: "Consensus logs pruned each epoch: 286 MB per validator, was 3.1 GB", state: "met", evidence: "measured on the testnet", href: `${repo}/docs/mainnet-v1-plan.md` },
  { n: 10, name: "Halt recovery", line: "No cheap committee capture, and a tested way back from a halt", state: "met", evidence: "attacknet + runbook", href: `${repo}/docs/runbooks/chain-halt.md` },
];
