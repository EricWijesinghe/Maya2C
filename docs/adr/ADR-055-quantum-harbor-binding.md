# ADR-055: Quantum Harbor, step one: a signed, timestamped binding from a classical key to a PQ vault

**Status:** Accepted 2026-10-19 (Eric: "all decisions are approved, take the best recommended"), with the answers below. Built after the mainnet launch gates (band P5).

**Answers:**
1. After the launch gates, not before: it is a differentiator, not a gate.
2. The anchor: an `AnchorAttestation` root published under a project DID. No new transaction kind, so no consensus change.
3. `k256` goes into `quantum-harbor` only, after its audit history is checked and it gets a `cargo vet` entry.

## Context

Queue item 5 asked for a demo that "migrates a classical key to a PQ account". Two pieces exist already:

- `crates/quantum-harbor`: says whether a Bitcoin output or an Ethereum account has its public key exposed (verified on Bitcoin's real genesis coinbase).
- Vault accounts (ADR-030, `TxKind::Vault`): a Maya2C account under hybrid PQ keys whose large withdrawals wait and can be cancelled by a guardian.

What does not exist, and cannot without a bridge, is moving BTC or ETH onto Maya2C. A bridge is outside ADR-016's launch scope. `docs/prior-art/quantum-harbor.md` says "nothing may be claimed", and that stays true after this ADR.

## Decision (proposed)

Build the part that is real today: **proof of prior ownership.**

1. `maya2c harbor check <btc-address|eth-address>`: the existing exposure answer, in plain words.
2. `maya2c harbor bind`: the holder signs one statement with their **classical** key (Bitcoin BIP-322 message signature, Ethereum EIP-191 `personal_sign`), and the Maya2C vault key signs the same statement:

   ```text
   maya2c-harbor-binding v1
   classical  <chain>:<address>
   vault      <maya2c vault address>
   chain_id   <maya2c chain id>
   ```

3. The binding's BLAKE3 hash is anchored on Maya2C, so it has a block timestamp before any quantum computer exists. The full statement is published off-chain: a file, and an explorer page later.

Why it matters: if Bitcoin or Ethereum ever adopt a migration rule for exposed coins (BIP-360 P2QRH work, Ethereum's proposed post-quantum emergency fork), a holder can show they controlled the key *before* it could be forged. Maya2C records this; it does not decide anything about another chain.

**No consensus change.** Classical signature checking (`k256`) lives in `quantum-harbor`, an extended-tier crate, never in the node. The anchor uses an existing transaction kind; which one is open question 2.

## Not claimed

- No BTC or ETH moves. Nothing is "protected" on its own chain.
- A binding made *after* a key is already forged proves nothing; the timestamp only helps if it predates that.
- No "first" or "only": others work on the same problem (see the prior-art file, to be updated with a dated search before anything is published).

## Questions for Eric

1. Build it now (P5, after the launch gates), or after mainnet?
2. The anchor: an `AnchorAttestation` root (needs a DID), or a new small "data anchor" transaction kind (a consensus change with its own ADR).
3. Add `k256` (RustCrypto; its audit history to be checked before adding) to `quantum-harbor` only.
