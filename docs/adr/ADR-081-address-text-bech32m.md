# ADR-081: Addresses have a Bech32m text form with a network prefix and a checksum

**Status:** Accepted, 2026-11-07. Implemented for the crate, the node RPC and the `maya-address` tests. Wallet GUI, explorer, faucet, pool, hub, chat, CLI output and the Go SDK are not converted yet (see "Not done").
**Date:** 2026-11-07
**Related:** [ADR-005](ADR-005-dependency-unification.md) (no new dependency), [ADR-036](ADR-036-genesis-id-signature-binding.md) (genesis id binds signatures, not the network name), `crates/node/src/crypto/hybrid.rs` (derivation, unchanged), `spec/address-vectors.json`

## Context

An account address is `blake3("custom-l1-node.address.v3" || ml_dsa_pk || slh_dsa_pk)`,
32 bytes, written as 64 hex characters. Hex carries no network and no
checksum. Two failures follow:

- An address for another network (or a truncated or transposed one) is
  accepted by the RPC and sent to. The chain has no way to tell.
- A mistyped character yields a different valid-looking account. The only
  check today is the wallet GUI's mixed-case checksum, which is display-only
  and absent from the RPC, the CLI and every SDK.

Mainnet genesis is planned for early December. The text form is the thing
people paste, so it has to be fixed before then.

## Decision

1. **The on-chain account id does not change.** State keys, signatures, the
   derivation and every consensus structure stay byte-identical. Only the text
   encoding is new. This is why no genesis or state migration is needed.
2. **Encoding: Bech32m (BIP-350).** Layout: `<hrp>1` + version symbol (0) +
   52 payload symbols (256 bits, last group zero-padded) + 6 checksum symbols.
   A 32-byte id is 64 characters on mainnet and 65 on testnet and devnet. Bech32m's checksum detects every
   single-character error and any burst up to five characters. The decoder
   requires the padding bits to be zero, so a non-canonical string cannot
   decode to a valid id.
3. **Network prefixes (HRP):** `maya` mainnet, `tmaya` testnet, `dmaya`
   devnet. The HRP is the network check: decoding for the wrong network returns
   `WrongNetwork` and names both prefixes.
4. **HRP per network comes from the chain id.** `Network::for_chain_id` maps a
   registered chain id to its network. `maya-testnet-1` is the only registered
   id. An unregistered id has **no text form**: the node serves hex only and
   refuses text input. A guessed prefix would let an address look valid on a
   network it does not belong to. The mainnet chain id is registered in the
   same change that fixes mainnet genesis.
5. **Version symbol first.** It lets a future format be added without changing
   the prefix scheme. Only version 0 is defined; any other is refused.
6. **Input policy during testnet.** Hex stays accepted on every input that
   accepts it today. Text is also accepted. Output is hex in every existing
   field. The new text form is additive (`address_text` in `get_balance` and
   `get_account_at_tip`), so explorer, hub and SDK clients keep working. Hex
   output moves to text only after every consumer has switched; that is a
   separate decision, not part of this ADR.
7. **No new dependency.** The crate `crates/address` (`maya-address`) depends
   only on `thiserror`. BIP-350 is about 150 lines and is checked against the
   official vectors, so there is no new supply-chain entry to vet.
8. **One shared vector file.** `spec/address-vectors.json` is read by the Rust
   crate, the TypeScript SDK and the Python SDK. A change to the text form
   fails in every language at once. The file was produced by an independent
   reference script, not by the Rust code under test.

## Consequences

- A mistyped address is refused by the RPC rather than sent.
- Clients that pass hex are unaffected.
- The text form is 64 characters (mainnet) or 65 (testnet, devnet). Some UIs
  must allow the longer string.
- The HRP is part of the address. Changing the prefix of a network later
  would change every text address of that network. The three prefixes above
  are therefore final once mainnet genesis is signed.

## Not done in this change

Wired, now: `maya-address` crate; node RPC input (`get_balance`,
`get_balance_at_height`, `get_account_at_tip`, and the miner param of
`get_mining_work`) and the additive `address_text` reply field on
`get_balance` / `get_account_at_tip`; the shared vectors.

Not wired (list from `rg`, to convert in follow-ups): wallet GUI core
(`apps/wallet-gui/core/src/payment.rs` `normalize_address`/`decode_address`,
`confirm.rs` `checksummed`); `bins/l1-wallet` `decode_address` and its `address`
print; `bins/maya2c-cli` (`dev.rs`, `main.rs` address prints);
`apps/faucet`, `bins/pool-service`, `apps/hub/src-tauri`, `apps/chat*`
(`hex::encode` of addresses); `sdks/sdk-ffi`, `sdks/sdk-wasm`; `sdks/go`;
`crates/node/src/genesis.rs` `decode_address` (genesis config is being edited
by another change and was left alone); genesis and deploy docs.
TypeScript and Python SDKs gained the encoder and decoder with this change
(`sdks/sdk-js/src/address.ts`, `sdks/python/maya2c_sdk/address.py`) and read the
shared vectors. The Go SDK does not.
