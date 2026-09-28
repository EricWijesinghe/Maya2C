# 24 — The developer platform

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 24

> DONE WHEN: dev, fork mode, replay, and the time-travel debugger work end to
> end; the MCP server passes its task set; SDKs are generated from one
> definition; the developer study is recorded.

| Condition | Result |
|---|---|
| `maya2c dev`, fork mode, deterministic replay, time-travel debugger | **all four work end to end** (2026-09-27), in `bins/maya2c-cli`, with the limits in § The `maya2c` CLI |
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
- Lazy state loading for fork mode: **a finding, not a gap** (see "Source
  lines and gas per line" below).
- Mapping a guest trap (`unreachable`, a divide by zero) to its source
  line. Host calls map; a trap never reaches the host.
- The VS Code extension has been tested through the adapter over stdio, not
  by driving the VS Code UI.
- The one-command test framework with coverage and gas snapshots.
- Protocol revenue share for developers. The prior art is recorded (not
  searched) in `docs/prior-art/developer-revenue-share.md`; there is no fee
  to share while the fee market is inactive.

## The `maya2c` CLI (2026-09-27, Windows workstation, debug build)

`bins/maya2c-cli`, binary `maya2c`, was an empty placeholder. It now has four
commands, all tested against real processes and the real `nft-game` module:

```
$ cargo test -p maya2c-cli -- --nocapture
maya2c dev: chain up — RPC in 1.1009455s, first block in 1.9078393s
  deployed  …contract.wasm -> contract e63294cd… in 1.03626s
  deployed  …contract.wasm -> contract 722548f6… in 1.8668176s
test dev_starts_a_funded_chain_and_redeploys_on_save ... ok
replayed block 6 in 58.9086ms; forked at 6 in 39.6629ms
test a_block_replays_exactly_and_a_fork_diverges_locally ... ok
test a_mint_can_be_stepped_forward_and_back ... ok
test a_refused_call_is_still_recorded_up_to_the_refusal ... ok
```

- **`maya2c dev [--watch x.wasm]`** starts a one-validator DAG-BFT chain
  with pre-funded accounts (ordinary `l1-wallet` keystores). It starts the
  explorer if it sits next to the binary, and deploys the watched module on
  start and on every save. Measured: RPC 1.10 s, first block 1.91 s, under the
  brief's 2 s; save-to-committed deploy 1.0–1.9 s.
- **`maya2c debug <wasm>`** runs a call once, recording every host
  interaction (`maya_vm::trace`). It then steps forward and backward
  (`n`, `b`, `g <k>`), showing storage and events as they stood at that step.
  Backward is exact and re-executes nothing.
- **`maya2c replay --from URL --genesis FILE <h>`** builds a local copy up to
  `h - 1` and executes block `h`. The chain refuses a block whose state root
  it does not reproduce, so success *is* the "exact same result"; the block's
  balance changes are printed.
- **`maya2c fork --from URL --genesis FILE`** copies the source to its tip,
  then serves JSON-RPC locally and makes its own blocks from its own mempool.
  The test sends a transaction to the fork that the source never sees.

**Limits, stated:**

- The debugger steps **host calls**, not source lines. The VM has no
  per-instruction hook, contracts carry no DWARF, and gas is known per call.
  VS Code reaches it through `maya2c dap` (below). Cross-contract
  calls do not exist in the VM, so there are none to step into.
- Fork and replay copy state **eagerly**, not lazily. The default is a full
  sync that executes every block from genesis; `--snapshot-depth` starts from
  the source's snapshot instead, but a source serves only its newest one, so
  that path cannot replay below it.
- On a DAG-BFT network the copy verifies execution (every state root
  reproduced) and header linkage, **not the committee's certificates** for
  each header. A malicious source could serve a self-consistent chain that
  was never certified. Proof-of-work headers do carry their verification.
- Replay is per block. Stepping through one transaction of a replayed block
  in the debugger is not wired; the debugger runs local calls.


## The debugger in VS Code: `maya2c dap` (added 2026-09-28)

`bins/maya2c-cli/src/dap.rs` is a Debug Adapter Protocol server in Rust over
the same recording `maya2c debug` uses. `editors/vscode` is a manifest-only
extension: VS Code starts `maya2c dap` and needs no TypeScript. It
advertises `supportsStepBack`, so **Step Back** and **Reverse Continue** work
and are exact. The frame's source is the recording, one line per host call;
Variables shows storage at the cursor, events so far and the call; the debug
console takes the CLI commands. Breakpoints are answered as unverified,
because steps are host calls, not source lines.

The test starts the real binary and speaks framed DAP to it over stdio,
against `nft_game.wasm`: it launches, steps forward, clamps at the end,
steps back so a written key reads `absent` again, jumps with goto, evaluates
`l` and disconnects.

```
$ cargo test -p maya2c-cli --test dap_tests
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

## Source lines and gas per line (2026-09-28)

The debugger now names the source line of every host call and the gas
spent reaching it, for contracts built with debug info (`-g`).

- **In the VM** (`crates/vm`): `Vm::execute_traced` installs a store call
  hook only on this path. At each host call the hook records the wasm module
  offset of the calling instruction and the fuel left. A review confirmed:
  - the consensus path is bit-identical, since no hook is installed there
    and `call-hook` is a cargo feature, not a configuration setting;
  - the captured frame is the call instruction, not the one after it;
  - no call site is attributed to the wrong step.
- **In the debugger** (`bins/maya2c-cli/src/lines.rs`): the contract's own
  DWARF line table, read with `gimli`, maps each offset to `file:line`.
  Each sequence's end is recorded, after the review showed an offset
  between two functions would otherwise take the earlier one's last line.
  The CLI's listing shows the line, `profile` prints gas by line, and the
  VS Code adapter opens the real source file at the line.

The test compiles a small contract with `rustc -g` for wasm32 and debugs it:

```
$ cargo test -p maya2c-cli --test source_lines_tests -- --nocapture
step 1 at probe.rs:12, step 2 at probe.rs:13
gas by line: {("probe.rs", 12): 4128, ("probe.rs", 13): 21}
test result: ok. 1 passed
```

Tracing costs no gas (`a_traced_call_burns_identical_gas_and_records_distinct_sites`).
Gas is attributed to the host call it leads up to, so line 12's figure
includes the call's setup. Gas after the last host call belongs to no line.

**Lazy fork state is a finding (Standing Order 9).** The brief asks fork
mode to "lazy-load state on demand". A fork makes its own blocks, and the
node refuses any block whose state root it does not reproduce (invariant
24). The root is computed over every account, and a node that fetched
accounts only as they were touched cannot compute it. A lazy fork would
have to run with root checks off, which is the one check fork mode exists
to keep, or compute roots over partial state, which would make its roots
differ from the source chain's. Fork mode stays eager (full copy, or from
the source's snapshot with `--snapshot-depth`).

