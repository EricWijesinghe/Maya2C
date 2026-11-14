# 10. Contract gas — v0.1.0

Caps and prices for contract work (ADR-142). Reference: `crates/vm/src/gas.rs`
(prices), `crates/governance/src/params.rs` (cap parameters). Node:
`StateDB::call_contract`, `StateDB::deploy_contract`,
`state::vm_exec::admit_call_gas`, mempool admission. Vectors:
`spec/tests/vm_gas.json`, replayed by `crates/node/tests/vm_gas_tests.rs`.

Gas is wasmtime fuel. It is not billed in coin: it bounds how long one call and
one block may run. A call that runs out of gas fails, and a failing
transaction makes its block invalid (STF-8), so a block producer leaves it out.

- **GAS-1** A contract call whose `gas_limit` is above `min(vm.max_tx_gas, vm.max_block_gas)` makes its block invalid (`GasAboveCap`) and is refused at mempool admission, before its signature is checked.
- **GAS-2** A block's contract gas starts at 0. A call whose `gas_limit` is above `vm.max_block_gas` minus the block's contract gas so far makes its block invalid (`BlockGasExhausted`); a call that applies adds its `gas_used` to it. A deployment adds `deploy_fuel(len)` under the same rule, and one above `vm.max_block_gas` is refused at admission. Sealed actions opened at their reveal height count in the block that opens them.
- **GAS-3** A call pays `call_fuel(len, warm) = 50,000 + b × len` before its first instruction, where `len` is the module's byte length and `b` is 450 for the first call to that contract in the block and 1 for later calls in the same block; `deploy_fuel(len) = call_fuel(len, cold)`. A `gas_limit` below `call_fuel` is out of gas. The block's first call to a contract is decided by the order of the block's calls, never by a cache.
- **GAS-4** Every `env` host function charges 600 before its body, except: `storage_read` charges `5,000 + key_len` before the lookup and `value_len` after it; `storage_write` charges `5,000 + 240 × (key_len + value_len)` before reading either; `emit_event` charges `600 + topic_len + data_len` before copying. Lengths are the call's arguments (unsigned), sizes over `MAX_KEY_BYTES` (128), `MAX_VALUE_BYTES` (4,096) or `MAX_EVENT_BYTES` (4,096) are refused before any charge, and a charge larger than the fuel left traps out of gas. *(positive only: defines a value)*
- **GAS-5** `vm.max_tx_gas` (parameter tag 17) and `vm.max_block_gas` (tag 18) start at 250,000,000 and move only by a governance proposal, within `[239,979,600, 500,000,000]`, checked when proposed and again when executed. The floor is `call_fuel(524,288, cold) + 4,000,000`: the largest module stays callable.
- **GAS-6** Nothing is refunded: unused gas is never charged, and an overwrite or an empty write pays like any other write. *(positive only: no input is rejected by this rule)*

## Gaps

- The prices are calibrated on one machine (ADR-142); a slower validator
  validates the same block more slowly, it does not validate it differently.
- No fee is paid per unit of gas; the fee market prices bytes (FEE-5).
- The contracts layer of the state root is folded over all contract code and
  storage each block (ROOT-5), which no gas price bounds. ADR-142 records it.
