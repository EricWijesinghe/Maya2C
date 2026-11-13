# ADR-084: The MCP server reads the chain and drafts transfers; it never signs or submits

**Status:** Accepted, 2026-11-10
**Date:** 2026-11-10
**Related:** [ADR-077](ADR-077-sentinel-recovery-network.md) ("AI proposes, never acts"), [ADR-081](ADR-081-address-text-bech32m.md), [ADR-083](ADR-083-hub-wallet-and-chat-pages.md); `bins/maya2c-mcp`, `docs/mcp-server.md`

## Context

`maya2c-mcp` let an assistant call any node method through `rpc_call`, and
allowed state-changing methods (`send_raw_transaction`, `submit_block`) to a
node on loopback. That let an assistant submit transactions, which contradicts
the rule that AI proposes and never acts.

## Decision

1. **Six read methods, one allowlist.** The server sends exactly
   `get_chain_info`, `get_balance`, `get_block_by_height`, `get_validators`,
   `get_supply` and `get_fee_info`. The allowlist is checked at the single
   point where requests leave the process (`bins/maya2c-mcp/src/node.rs`), and
   a test pins it. (Amended by v3 P6, PR #482: a seventh read, `get_balance_at_height`, for `compare_account`.)
2. **No signing tool, no submit tool.** `rpc_call` is removed. A test calls
   every listed tool and checks that the node receives nothing outside the allowlist
   methods.
3. **`draft_transfer` returns an unsigned transaction.** It is built with
   `Transaction::new` and serialised with an empty signature. The sender's
   public key is written at signing time, so the draft cannot carry a
   signature. A test shows a wallet can sign the draft and verify it.
4. **Inputs are validated before the node is asked.** Addresses are
   `maya1...` text or 64 hex characters; amounts are coins with at most 24
   decimal places, never rounded; request lines are capped at 256 KiB.
5. **Client-side rate limit** at the node's own default (50/s, burst 100), so
   the server does not trip the node's per-IP limit.

## Consequences

- An assistant cannot move funds. A person signs with their own wallet.
- `get_tx` needs the block height, because the node has no lookup by id.
  Adding one would be a node change, which is frozen until genesis.
- The rate limit and the size cap are chosen values, not measured ones. The
  size cap is marked as a shortcut in the code.
