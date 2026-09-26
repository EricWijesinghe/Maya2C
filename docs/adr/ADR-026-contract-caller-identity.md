# ADR-026: Contracts need to know who called them

**Status:** Proposed. Not decided, because it changes the consensus-visible
host surface.
**Date:** 2026-09-26

## Context

Master Prompt 30 asked for five reference applications. Writing the NFT game
(`contracts/nft-game`) found the following. **A Maya VM contract cannot learn
who invoked it.**

- `HOST_FUNCTIONS` (`crates/vm/src/runtime.rs`) has no `caller`, `sender` or
  `origin`.
- `StateDB::call_contract` (`crates/node/src/state/vm_exec.rs`) receives the
  `caller` address. It uses the caller only to preload a balance, and passes
  `call.input` through unchanged.
- Nothing in the docs, the spec or `llms.txt` said so before this ADR.

Every contract that owns something depends on caller identity: a token balance,
an NFT, a DAO vote, a vault, an admin key. In Solidity that check is
`msg.sender`, and Anchor's `Signer` account plays the same role. Without it, a
port has to take the acting account as an argument, and an argument is
whatever the caller writes. `crates/reference-apps/tests/nft_game.rs` shows the
consequence: `anyone_can_move_anyones_token` moves Alice's token to Mallory
with no signature from Alice.

The only contract in the tree before this, `contracts/token-swap`, is a pool
with no per-user state, so the gap never showed.

## Options

1. **A `caller(out_ptr) -> i32` host function** that writes the 32-byte
   address of the account that signed the transaction. The node already has
   the address. This is the smallest change, and it is what every porting
   guide will assume.
2. **Signature checks inside the contract**: the contract verifies an ML-DSA
   signature over its input. This needs a verify host function (a PQ verify
   in WASM costs millions of fuel), and it duplicates the transaction's own
   signature.
3. **Capabilities** (`contract-safety`, MP23), where the runtime hands the
   contract a token proving authority. This is the strongest model, but
   nothing in the VM implements it yet.

## Proposed decision

Option 1, with the following conditions:

- It activates at a written activation height. A new import is a new
  consensus rule, and a node that lacks it rejects every contract that uses it
  (the `HOST_FUNCTIONS` deploy-time check).
- Until that height, `validate` refuses a module that imports `caller`.
- It has no `origin` counterpart. There are no contract-to-contract calls
  (`env.call` is deliberately absent), so the caller is always the
  transaction signer. That is also why the tx.origin phishing class does not
  exist here.
- `nft_game.rs::anyone_can_move_anyones_token` is inverted to prove the fix.

## Consequences if deferred

- No reference contract with ownership can ship.
- The migration kits (`docs/migration/`) must tell porting teams that this is
  the first thing that will not port.
- The MP30 NFT template stays blocked.
