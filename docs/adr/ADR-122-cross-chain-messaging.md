# ADR-122: Generic cross-chain messages over verified headers

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled. Inbound (Ethereum → Maya2C) only; see "What this is not".
**Date:** 2026-11-12
**Related:** parity row P-34, [prior-art/cross-chain-messaging.md](../prior-art/cross-chain-messaging.md), [ADR-065](ADR-065-cross-chain-asset-routes.md), [ADR-026](ADR-026-contract-caller-identity.md), [ADR-114](ADR-114-contract-timers.md), [ADR-120](ADR-120-cross-chain-token-standard.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 11, 24, 25, 28, 33

## Context

`crates/interop` is RESEARCH and nothing in consensus calls it
(`features.toml:3286-3294`, `lib.rs:1-17`). It holds: Ethereum execution
header hashing (`eth.rs:129-131`, RLP + Keccak, with `receipts_root` in the
header at `eth.rs:24`); an Altair/Electra beacon light client that verifies a
sync-committee signature over an attested header, the finality branch
(gindex 169) and the execution payload branch (gindex 25)
(`beacon.rs:7-19`, `verify_bootstrap` `:264`, `verify_finality` `:286`); and
per-route caps that grow with incident-free time (`routes.rs:14-98`). What
it lacks is stated in its own docs: committee handover between periods
(`beacon.rs:21-23`: "one bootstrap serves one 8,192-slot period"), any
receipt or log proof (ADR-065: "no receipt or log proof, so no asset route"),
and any ZK wrapping. The sync committee signs with BLS12-381, which is not
post-quantum; the crate says so (`lib.rs:15-17`).

ADR-065 reserved a bridge layer `x:` (`commitments.rs:252`) and chose, in
order: swaps only; Bitcoin inbound with disclosed custody; Ethereum receipt
proofs last, "until options 1 and 2 have run on the public network". This
ADR is the design for that third option, written now so the token standard
(ADR-120) has a route to stand on; its activation is after ADR-065's option
2 by construction (below).

IBC verifies packets with on-chain light clients and gives per-channel
ordering and exactly-once delivery; Avalanche ICM and Chainlink CCIP use
validator or oracle sets; LayerZero uses configurable verifiers (prior-art
file).

**Finding that contradicts the brief.** The row says "CCIP/ICM-class
messaging over verified headers" as one feature. Only one direction can be
built on verified headers from this tree. **Outbound** (Maya2C → Ethereum)
needs an Ethereum contract that verifies Maya2C finality, which is `n − f`
ML-DSA-87 signatures of about 4.6 KB each over a DAG-BFT vote
(`consensus/bft/auth.rs`); no EVM precompile verifies ML-DSA today, and a
Solidity verifier would cost more gas than a block holds. Outbound therefore
needs either a ZK proof of the certificate (ADR-079 research) or a
destination-side verifier set, which is the trust model ADR-065 refused.
This ADR builds inbound and exports the outbound *record* only.

## Decision

**A Maya2C contract or account receives a message that an Ethereum contract
emitted, if and only if a finalized Ethereum execution block whose
`receipts_root` contains the emitting log is proven under the beacon light
client's committed state. Delivery is once per `(source chain, source
contract, nonce)`, in nonce order per source, and a message is executed by
calling its target as a timer runs (ADR-114): `caller() = None`, failure a
no-op, the receipt kept.**

### Light client in state (invariant 25)

Prefix `x:lc:eth:` in the bridge layer `StateLayer::Bridge` (`x:`, reserved
by ADR-065; the layer is created by whichever of ADR-065 option 2 and this
ADR lands first):

- `x:lc:eth:committee:` ‖ `period(8 BE)` → the sync committee root for that
  period, written by `verify_bootstrap` at genesis-of-the-route and
  thereafter by the **handover** update (below).
- `x:lc:eth:finalized` → `{ slot: u64, block_number: u64, block_hash: [u8;
  32], receipts_root: [u8; 32] }`, monotonic in `slot`.

`TxKind::SubmitEthUpdate(Box<FinalityProof>)` carries a `FinalityProof`
(`beacon.rs:236`) plus the full execution header (`eth.rs:12`), and, for a
period boundary, the `next_sync_committee` branch (gindex 55 at Electra; the
constant is pinned by a vector, never computed in the node). The node runs
`verify_finality` against the committee in state, checks `eth::hash(header)
== block_hash`, and writes `finalized` and, if present, the next committee.
Anyone may submit; a proof that does not verify is a refused transaction
(invariant 7). Admission is bounded: at most `MAX_ETH_UPDATES_PER_BLOCK =
2`, because a 512-key `FastAggregateVerify` is real work and the fee is
bytes only (same reason ADR-114 charges a flat fee; question 1).

Committee handover is **new code in `crates/interop`**, with its own
vectors from a real Ethereum period boundary, before anything here is wired.
That dependency is listed in the test plan, not assumed.

### Messages

- `x:msg:in:` ‖ `src_chain(8 BE)` ‖ `src_contract(20)` ‖ `nonce(8 BE)` →
  `Receipt { delivered_at: u64, target: [u8; 32], status: u8 }`.
- `x:msg:next:` ‖ `src_chain` ‖ `src_contract` → the next nonce expected.
- `x:msg:route:` ‖ `src_chain` ‖ `src_contract` → `Route` (`routes.rs:14`):
  a count cap per window that grows with clean days; an `incident` (a proof
  that failed verification after admission, which cannot happen by
  construction, or a governance pause) resets it.

`TxKind::DeliverEthMessage { block_number, receipt_proof: Vec<Vec<u8>>,
log_index: u32 }`: the node checks `block_number <= finalized.block_number`
and that the submitted header's `receipts_root` equals the one in state for
that block (the finalized record keeps only the latest; an older block is
proven by a chain of `parent_hash` back from it, at most
`MAX_HEADER_CHAIN = 64` headers, or it waits). The Merkle-Patricia receipt
proof is verified in a new `interop::receipt` module (RLP decode, Keccak
paths, bounded depth 64). The log must be from `src_contract`, with topic 0 =
the fixed `MessageSent(uint64 nonce, bytes32 target, bytes payload)`
selector, and `nonce == x:msg:next`. `payload.len() <= MAX_MESSAGE_BYTES =
4 KiB`.

Execution: if `target` names a contract, run `call_contract` with input
`src_chain ‖ src_contract ‖ nonce ‖ payload`, `caller() = None`, gas
`VmDefaultGasLimit`; a trap is `status = 1` and a no-op. If `target` is an
account, the message is a receipt only (a wallet reads it). Either way
`next += 1` and the route's window count is charged. ML-DSA, BLAKE3 and
Keccak are deterministic; no float, no clock (Standing Order 4).

### What this is not

- Not outbound. A Maya2C contract that wants to send writes `x:msg:out:` ‖
  `nonce` → `{ dst_chain, dst_contract, payload_hash }` through a host
  function `send_message` (fuel first, invariant 21). That record is under
  the state root, so a `StateLayer::Bridge` proof (`state/proof.rs:311`)
  against a finalized header is the exportable message. **What verifies that
  proof on Ethereum does not exist**, and this ADR does not claim it does.
- Not post-quantum on the Ethereum side: the sync committee is BLS12-381.
  The route cap bounds what a forged committee signature could deliver per
  window; every user-facing string says "inherits Ethereum's signature
  security" (Standing Order 10 and ADR-065's disclosure rule).
- Not a trusted party (invariant 11): the only trust is the bootstrap root,
  which genesis-of-the-route writes down and which is the same trust an
  Ethereum light client has.

## Alternatives considered

- **Validator-set attestation (ICM, CCIP).** Faster to build, and the
  bridge-trust row ADR-065 refused. Rejected.
- **ZK proof of Ethereum finality (zk light clients).** Smaller proofs, no
  BLS in the node; the provers are not post-quantum either and none exist
  in this tree. A later ADR may wrap the same state records.
- **IBC-style channels with handshakes.** The `(src, nonce)` ordered stream
  is one IBC ordered channel per source contract without the four-step
  handshake, which exists to let both sides verify each other; this ADR has
  one side.
- **Deliver by `CallContract` from a relayer with `caller() = relayer`.**
  Then the contract cannot tell a proven message from a relayer's claim.
  `caller() = None` plus the committed receipt is the distinction ADR-114
  already uses.
- **Unordered delivery.** Simpler, and lets a relayer reorder two messages
  from one source; a nonce stream is cheap.

## Consequences

- Inbound Ethereum events become a proven input to contracts; the token
  standard (ADR-120) can mint against them.
- New: `interop::receipt` (MPT proofs), committee handover, two `TxKind`s,
  three prefix families under `x:`, one host function, `Module::Bridge`
  breaker arm, per-block update bound.
- Every update costs the network a 512-key BLS verification; `MAX_ETH_UPDATES_PER_BLOCK`
  bounds it and question 1 prices it.
- The outbound half is written down as not built, with its blocker named.

## Invariants touched

- **11**: no trusted party; the bootstrap root is genesis-of-the-route data.
- **24**: deliveries and updates in the only apply path.
- **25**: `x:` layer records; undo coverage.
- **7**: a failed proof is a refusal; a failed target call is a no-op with a
  receipt.
- **28**: `Module::Bridge`; a breaker stops updates and deliveries.
- **33**: `FinalityProof` ~ 512 pubkeys × 48 B + branches ≈ 26 KiB; a
  receipt proof ≤ 64 nodes; both inside `MAX_TX_BYTES`.
- **21**: `send_message` charges fuel first.

## Activation

`FeatureSet::XCHAIN_MESSAGING = 1 << 29` (claimed in `adr-claims.txt`),
compiled `XCHAIN_MESSAGING_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**, and never before
ADR-065's option 2 has run on the public network (ADR-065's own ordering).
Before activation both kinds are refused and `send_message` is absent from
the linker.

## Test plan

Prerequisites with their own red tests first: `crates/interop/tests/handover_tests.rs`
(a real period boundary from mainnet, committee N → N+1) and
`crates/interop/tests/receipt_tests.rs` (a real receipt proof against a
mainnet `receipts_root`; a tampered node; an over-depth path).

Then `crates/node/tests/xchain_message_tests.rs`:

- `a_message_from_a_finalized_block_is_delivered_once_and_the_receipt_is_committed`
- `a_message_with_the_wrong_nonce_waits` (refused, not dropped)
- `a_log_from_another_contract_under_a_valid_proof_is_refused`
- `an_unfinalized_block_number_is_refused`
- `a_trapping_target_is_a_no_op_with_status_one`
- `the_route_count_cap_refuses_the_n_plus_first_delivery_in_a_window`
- `the_third_update_in_a_block_is_refused`
- `a_reorg_restores_finalized_next_and_receipts`
- `a_block_whose_root_omits_a_delivery_is_refused` (invariant 24)

Vectors (`spec/interop/`): the committed finalized record; one delivery; the
outbound record and its layer proof. `cargo xtask spec-coverage` lists them.

## Open questions for Eric (economics only)

1. Who pays for a 512-key BLS verification per update: a flat `ETH_UPDATE_FEE`
   on the submitter (default), or a treasury subsidy (ADR-111) so updates
   keep flowing when no app needs one?
2. The route count cap's `base`, `per_clean_day` and `ceiling` at activation.
3. Should a delivery pay a flat fee to the target contract's timer escrow
   (ADR-114) so the contract's handling is prepaid, or is the deliverer's
   byte fee the whole price?
