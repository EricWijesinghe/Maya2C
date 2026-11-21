# ADR-098: Supply audit is a standalone binary that does not link the node

Status: Accepted (tooling only; no consensus, wire or state change)
Date: 2026-11-11

## Context

`docs/ecosystem/` catalog item `supply-audit` (finding F1 in the catalog-M
notes) asks that anyone can check no coins were printed. The check the chain
already makes is inside the node: `crates/node/src/state/invariant_guard/conservation.rs`
runs in every block. A node that is wrong about conservation would also be wrong when
it checks itself, and a tool that links the node inherits whatever the node
computes.

`get_supply` (`crates/node/src/rpc/market.rs`) reports a total and a
circulating figure at the tip. It cannot show that the mining reserve paid only
the scheduled emission, and it does not say how it summed the accounts.

## Decision

- A new binary, `bins/maya2c-audit` (`maya2c-audit supply`), reads only the
  public JSON-RPC and the genesis file. It depends on `maya-amount`,
  `maya-ledger-math` and `maya-spec-ref`. It does not depend on `maya-node`.
- The emission schedule is `maya-spec-ref::mining` (MINE-1 and MINE-2),
  written from `spec/09-mining.md` and sharing no code with the node.
- Every check reports PASS, FAIL, NOT VERIFIED or NOT APPLICABLE, with the
  heights it rests on. A height the node does not keep is NOT VERIFIED, never
  PASS. A transport or malformed answer stops the run (exit 2).
- The report lists what the tool does not check (genesis id, account
  enumeration, the committee/miner split within an epoch, the base fee, the
  shielded pool), and prints that list with every run.

## Consequences

- Emission is checked per epoch as `committee + paid shares`, with the shares
  bounded by the miners' budget. The RPC does not separate the committee part
  from the shares inside one epoch, so the check proves a range, not an exact
  figure. Exact per-share checks would need share records served by the node,
  which is a node change and waits for the freeze to lift.
- Unspent miner budget stays in the reserve (the open question in
  `spec/09-mining.md`). The report shows it as a number. Choosing burn or carry
  is a consensus decision and is not made here.
- The tool reads one height per RPC call. Auditing a long chain is slow; that
  is accepted for a verifier that is run, not served.
- Two copies of the address constants exist (node and tool). The tool's test
  pins them to their ASCII names; no test compares them to the node's bytes,
  since that would need node code.

## Alternatives rejected

- A subcommand in `maya2c-cli` or `xtask`: both link the node.
- Trusting `get_supply` alone: it is the node's own sum, which is the thing
  under audit.
