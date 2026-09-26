# 24 — The developer platform

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 24

> DONE WHEN: dev, fork mode, replay, and the time-travel debugger work end to
> end; the MCP server passes its task set; SDKs are generated from one
> definition; the developer study is recorded.

| Condition | Result |
|---|---|
| `maya2c dev`, fork mode, deterministic replay, time-travel debugger | **not built** |
| MCP server passes its task set | **yes**: `maya2c-mcp`, 10/10 tasks plus a refused public write |
| SDKs from one definition | **partly.** uniffi generates Python, Kotlin and Swift from one interface (`sdks/sdk-ffi`), and Python is compiled and tested there. TypeScript goes through `sdk-wasm`, separately. Go is a README only |
| Developer study (10 developers) | **not done**: it needs participants |

## MCP server

`bins/maya2c-mcp` speaks MCP's JSON-RPC over stdio. It has four tools:

- `decode_transaction` uses the node's own decoder;
- `review_intent` runs the clear-signing review;
- `explain_error` maps an error kind to plain language and its spec rule;
- `rpc_call` refuses state-changing methods unless the configured node is on
  loopback, so an assistant cannot write to a public network through it.

```
$ cargo test -p maya2c-mcp --profile ci -- --nocapture
test writes_to_a_public_node_are_refused ... ok
MCP task set: 10/10
test ten_assistant_tasks ... ok
test result: ok. 2 passed; 0 failed …
```

The tasks run the real binary over stdio:

- initialize and list the tools;
- decode a real conformance frame, and explain a malformed one;
- flag a fake-airdrop intent (ClaimMismatch + UnlimitedApproval);
- explain InvalidNonce, which cites STF-1;
- a read call to a node, and a loopback write;
- an unknown tool, and an unknown method.

The RPC side is tested against a fake local node, not a running `maya2c-node`.

**The brief's measure of AI coding success was not run**: "an AI assistant
completes 20 standard contract tasks". That needs the contract toolchain
(templates, deploy) that does not exist yet. `llms.txt` at the repository
root lists the facts an assistant gets wrong without them: the address
derivation, the 13 KB transfer, no memo, no decimals, the inactive fee
market, v7/v8 active from genesis.

## Not done

- A language ADR with measured Wasm size and gas per language. AssemblyScript
  and TinyGo toolchains are not installed here.
- The instant local loop, fork mode, replay, and the time-travel debugger
  with a VS Code extension.
- The one-command test framework with coverage and gas snapshots.
- Protocol revenue share for developers. The prior art is recorded (not
  searched) in `docs/prior-art/developer-revenue-share.md`; there is no fee
  to share while the fee market is inactive.
