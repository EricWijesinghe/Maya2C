# apps/portal

Empty. No portal exists in this tree.

The foundation brief lists `portal` under `apps/`. The brief also says, in its
first line, **"Do NOT build product features in this phase"** - so this is the
place, not the thing.

What already serves adjacent purposes, so a portal is designed against them
rather than duplicating them:

| Surface | What it is |
|---|---|
| `apps/explorer` | the chain explorer - blocks, transactions, accounts |
| `apps/dashboard` | a Leptos operations page over `crates/telemetry` |
| `apps/faucet` | testnet funding, with the rate limits invariant 16 describes |
| `crates/api-gateway` | the REST and GraphQL surface any portal would call |

A portal that is a fifth web surface with its own copy of the RPC client is
worth less than one that is a shell over `api-gateway`.
