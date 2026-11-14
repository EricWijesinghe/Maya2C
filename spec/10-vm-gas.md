# 10. Contract gas — v0.2.0

Caps and prices for contract work (ADR-142). Reference: `crates/vm/src/gas.rs`
(prices), `crates/governance/src/params.rs` (cap parameters). Node:
`StateDB::call_contract`, `StateDB::deploy_contract`,
`state::vm_exec::admit_call_gas`, mempool admission. Vectors:
`spec/tests/vm_gas.json`, replayed by `crates/node/tests/vm_gas_tests.rs`.

Gas is wasmtime fuel. It is not billed in coin: it bounds how long one call and
one block may run. A call that runs out of gas fails, and a failing
transaction makes its block invalid (STF-8), so a block producer leaves it out;
the gas the producer spent finding that out stays counted (GAS-2).

- **GAS-1** A contract call whose `gas_limit` is above `min(vm.max_tx_gas, vm.max_block_gas)` makes its block invalid (`GasAboveCap`) and is refused at mempool admission, before its signature is checked.
- **GAS-2** A block's contract gas starts at 0. A call whose `gas_limit` is above `vm.max_block_gas` minus the block's contract gas so far makes its block invalid (`BlockGasExhausted`); a call that applies adds its `gas_used` to it. A deployment adds `cold_call_fuel` (GAS-3) under the same rule, before its module is compiled. Revealed sealed actions count against a budget of their own, `sealed_gas_budget = vm.max_block_gas / 4`, which starts at 0 when the block's revealed actions start (after every plaintext transaction), which they share in envelope-key order, and which stands in for `vm.max_block_gas` in GAS-1 and in this rule while they run; a revealed call or deployment that fails is consumed and adds the gas it used: a deployment its whole `cold_call_fuel`, or `50,000 + len` when GAS-3's shape bounds refuse it; a call whose `gas_limit` is below its GAS-3 price `50,000 + len`, where `len` is its module's length (the code was read and, cold, scanned before the refusal); a call that ran, its failure gas (below); and 0 a call refused by GAS-1, GAS-2 or for naming no contract. A call's failure gas is the fuel it ran, its upfront price included, plus `6,000` for every page of the most memory it held (GAS-7), not capped at `gas_limit`: a call that ran out of gas reports `gas_limit` plus its pages. A call that overflowed the native stack reports `gas_limit + 6,000 × min(256, (gas_limit − upfront) / 6,000)`, whatever ran before the overflow: where the stack overflows depends on frame sizes, so on the validator's ISA and compiler flags, and the figure must not. The block builder counts a failed call or deployment it trial-runs the same way: the gas it used stays spent for the rest of the block being built, so the builder never runs more than `vm.max_block_gas + 1,536,000` of failed and applied contract work together (the last failure may pass the budget by its memory), and a block it builds spends no more than `vm.max_block_gas`. The builder tries at most one failing contract call or deployment per sender: once one has failed after doing contract work (any of the charges above but 0), that sender's later contract transactions in the same selection are passed over and stay pooled; one refused before any contract work (a stale nonce, a replay) does not count.
- **GAS-3** A call pays before its first instruction, cold for the first call to that contract in the block and warm for later ones, decided by the order of the block's calls, never by a cache. Warm: `50,000 + len`. Cold: `cold_call_fuel = 50,000 + 450 × len + 125,000 × functions + 170 × locals + 30,000 × controls + 8,000 × calls + 2,500 × arity + 30,000 × loop_arity`, where `len` is the module's byte length, `functions` its imported and defined functions, `locals` the locals its functions declare, `controls` its `block`, `loop`, `if`, `else`, `br`, `br_if`, `br_table` and `return` operators, `calls` its `call` and `call_indirect` operators, `arity` the values carried, summed over every defined function's own type, every `block` and `if` (0 for an empty block type, 1 for a single value type, params plus results for a type index) and every call (params plus results of the callee's type), and `loop_arity` the values every `loop` carries, counted the same way. A `gas_limit` below the price is out of gas. A module is refused at deployment, before it is compiled, when a function type names more than 16 params and results together, when one function declares more than 1,024 locals, holds more than 2,000 control operators or more than 10,000 calls, when it holds more than 10,000 functions, or when its `cold_call_fuel` is above 235,979,600 (the cold price of a 524,288-byte module with no shape); a deployment over that price is refused at mempool admission too, after its signature.
- **GAS-4** Every `env` host function charges 600 before its body, except: `storage_read` charges `5,000 + key_len`, and 95,000 more when the read is cold, before the lookup, and `value_len` after it, where a read is cold when it is the block's first read of that committed key: not a key the block's applied calls or this call wrote or read before; `storage_write` charges `5,000 + 240 × (key_len + value_len)` before reading either; `emit_event` charges `600 + topic_len + data_len` before copying; `host_verify_zkml_proof` charges its own `verification_fuel` instead (`crates/vm/src/zkml.rs`). Lengths are the call's arguments (unsigned). A size over `MAX_KEY_BYTES` (128), `MAX_VALUE_BYTES` (4,096), `MAX_EVENT_BYTES` (4,096) or `MAX_EVENT_TOPIC_BYTES` (256), a 65th event, a pointer out of bounds or a missing memory export ends the call at once with that error, before that host call is charged; a charge larger than the fuel left traps out of gas.
- **GAS-5** `vm.max_tx_gas` (parameter tag 17) and `vm.max_block_gas` (tag 18) start at 250,000,000 and move only by a governance proposal, within `[239,979,600, 300,000,000]`, checked when proposed and again when executed. The floor is `235,979,600 + 4,000,000`: every module GAS-3 lets deploy stays callable by a plaintext call. A revealed sealed call is capped by `min(vm.max_tx_gas, sealed_gas_budget)` instead (GAS-2), so a cold sealed call to a module whose `cold_call_fuel` is above `sealed_gas_budget` (62,500,000 at the default) never applies: a `gas_limit` above the budget is `GasAboveCap`, one within it is below the price (`OutOfGas`), and the envelope is consumed either way. It can apply only warm, after a plaintext call compiled the module in the same block.
- **GAS-6** Nothing is refunded: unused gas is never charged, and an overwrite or an empty write pays like any other write. *(positive only: no input is rejected by this rule)*
- **GAS-7** A call may hold at most `min(256, (gas_limit − upfront) / 6,000)` pages of linear memory, its initial pages included, where `upfront` is its GAS-3 price; asking for more ends the call (`MemoryLimit`). Once it returns it pays 6,000 for every page of the most it held; if that takes it past `gas_limit`, it ran out of gas, and its failure gas (GAS-2) is the whole sum, past `gas_limit`. Growth past the module's own declared maximum is refused by the engine and is never held.

## Gaps

- The prices are calibrated on one machine (ADR-142); a slower validator
  validates the same block more slowly, it does not validate it differently.
- No fee is paid per unit of gas; the fee market prices bytes (FEE-5).
- The contracts layer of the state root is folded over all contract code and
  storage each block (ROOT-5), which no gas price bounds: its cost grows with
  total contract state, not with the block's gas. ADR-142 records it as open;
  it belongs with the incremental state root (ADR-062a), not with gas.
- A failing plaintext call is never included, so it pays no fee; GAS-2 bounds
  what failed calls cost a block's builder, not who pays for them (STF-8).
  One such call naming the cap, from one funded sender, leaves every contract
  call after it in the sub-DAG out, every block, for nothing: **a genesis
  blocker** (ADR-142, BACKLOG.md). Closing it needs failed calls included with
  their fee and a nonce bump, a change to STF-8 and an economics decision.
- An envelope does not state its inner call's `gas_limit`, so a seal is
  accepted without a gas check and reveals share `sealed_gas_budget` in
  envelope-key order: one failing reveal naming the whole budget leaves the
  later sealed calls at its height consumed (`BlockGasExhausted`). Recorded in
  ADR-142 as open.
- Recursion: where the native stack overflows depends on the ISA. GAS-2 makes
  a call that overflows report the same gas everywhere, but a call that
  recurses deep enough to overflow on one ISA and not on another still
  succeeds on one and fails on the other. That existed before ADR-142;
  `max_wasm_stack` is 512 KiB and Cranelift's ISA flags are not pinned.
- Golden gas values were produced on x86-64 Windows only; nothing has replayed
  them on aarch64 yet (CI job `vm-gas-aarch64` is the replay).
- Prices are measured on shapes, not proved: ADR-142 lists the shapes that
  were measured and the ones that were not.
