# Sealed mempool

**Status:** SHIPPED, optional per network, not post-quantum
([architecture-vision.md](architecture-vision.md) line 61; `features.toml` line 1906).
**Written:** 2026-11-12, from the code at `origin/master` 016ba40f. Every claim
below cites the file and line it comes from. Line numbers drift; the quoted
identifiers do not.

This is the document that `crates/node/src/genesis.rs:88` and
`crates/node/src/sealed/mod.rs:81-84` point to, and that
[ADR-025](adr/ADR-025-mev-position.md) line 36 recorded as missing
(`docs/audit/KNOWN_ISSUES.md` #13).

## What it does

A sender encrypts a transaction to a committee key. A block producer orders the
ciphertext without being able to read it. After the block commits, committee
members publish decryption shares, and the transaction executes at a later
height in an order that was already fixed.

The property, in the crate's own words: "ordering is committed before contents
are known" (`crates/mev/src/lib.rs:22`). It is not confidentiality, since
everything is public a block later, and it is not censorship resistance, since a
miner can drop a ciphertext it cannot read (`lib.rs:23-25`). It removes
content-dependent ordering, which is the power MEV extraction uses
(`lib.rs:25-26`).

The inequality it rests on: an envelope included at height `h` opens at
`reveal_height > h`, and the shares that open it are not valid before
`reveal_height` (`crates/node/src/state/sealed_exec.rs:4-11`;
`crates/node/src/core/sealed_payload.rs:54-57`).

### Lifecycle

1. **Seal.** Anyone calls `maya_mev::seal(committee, plaintext, aad)`
   (`crates/mev/src/cipher.rs:115`). It needs only the public key
   (`cipher.rs:103-105`). The scheme is hybrid: ristretto255 ElGamal transports
   a shared point, which keys ChaCha20-Poly1305 (`cipher.rs:10-13`). The AEAD
   nonce is derived alongside the key, so a fresh ephemeral scalar gives a fresh
   nonce (`cipher.rs:15-21`). The plaintext is a `TxKind`
   (`sealed_exec.rs:430-431`).
2. **Bind.** The associated data is `envelope_aad(id, reveal_height)`
   (`crates/node/src/core/sealed_payload.rs:165`), so a ciphertext lifted into
   another envelope or height does not open. The id is
   `derive_envelope_id(sender, nonce)` (`sealed_payload.rs:149`), a BLAKE3 hash
   the sender can compute before submitting.
3. **Submit.** `TxKind::Seal` reaches `stage_envelope`
   (`crates/node/src/state/settlement.rs:215`). The sender, fee, nonce and size
   are public; what the action does is hidden (`sealed_payload.rs`, doc on
   `SealedEnvelope`). The envelope is stored under `m:env:<reveal height><id>`
   (`crates/node/src/sealed/mod.rs:45,110-116`), height first, so a scan visits
   exactly what is due.
4. **Share.** At the reveal height each committee member computes
   `decryption_share` (`cipher.rs:250`) and submits `TxKind::RevealShare`
   (`settlement.rs:217`). Each share carries a Chaum-Pedersen proof that is
   verified at submission (`sealed_exec.rs:208-211,267`), so a bad share is
   attributed to a member index (`cipher.rs:170-180`).
5. **Open.** `settle_sealed` runs once per block (`sealed_exec.rs:294`, called
   at `crates/node/src/state/db.rs:918`). It visits due envelopes in storage-key
   order, height then id (`sealed_exec.rs:284-288`), combines the shares
   (`sealed_exec.rs:422`), decodes a `TxKind`, and executes it under the stored
   sender (`sealed_exec.rs:378-399`).

## Trust model

**The setup is trusted.** `Committee::generate(threshold, members)`
(`crates/mev/src/committee.rs:100`) draws a random secret, computes the public
key, splits the secret with Shamir sharing, and zeroizes the original
(`committee.rs:101-108`). Whoever runs it holds the whole key for the length of
the call (`committee.rs:77-85`). The genesis file carries only the public part:
threshold, encryption key and member verification keys
(`SealedGenesis`, `crates/node/src/genesis.rs:515-523`). A `MemberSecret` is
zeroized on drop and deliberately not `Clone` (`committee.rs:45-47`).

**What a compromised setup costs.** It does not let anyone forge a transaction,
spend a coin or change a balance, because opened transactions are still signed
by their senders and validated in full. It costs the MEV protection and nothing
else; the chain degrades to a plaintext mempool (`committee.rs:86-93`;
`lib.rs:60-61`).

**Share secrecy.** With `t` of `n` shares the key is rebuilt; with `t-1` nothing
is learned, in the information-theoretic sense (`crates/mev/src/shamir.rs:1-7`).

**It is a classical primitive.** ristretto255 falls to a quantum adversary. The
crate's argument for accepting that: confidentiality has a lifetime of roughly
one block, and an adversary able to run Shor's algorithm against a ciphertext
within one block interval has already broken more urgent things
(`lib.rs:52-58`). A break here costs MEV protection only; the rest of the chain
does not depend on this assumption (`lib.rs:60-61`). Post-quantum threshold
encryption is not adopted because the crate states no standardized, audited Rust
implementation exists (`lib.rs:58-60`).

**Producers.** The producer of the reveal block learns plaintexts while
building it and could add a transaction of its own. The revealed batch runs
after all plaintext transactions, so such a transaction executes before the
revealed one: the producer can back-run but not front-run
(`sealed_exec.rs:20-25`). The batch runs before the trading pass, so a revealed
swap clears at the same uniform price as every plaintext swap in the block
(`sealed_exec.rs:27-31`).

## The committee

- Record: `CommitteeRecord { encryption_key, threshold, members }`
  (`crates/node/src/sealed/mod.rs:87-94`), stored at `m:committee`
  (`mod.rs:42`) and installed at genesis by `seed_sealed_committee`
  (`sealed_exec.rs:77`, called from `genesis.rs:1301-1302`).
- Optional and absent by default: a chain without it has no `m:` records and
  keeps the state root it had before the feature existed (`genesis.rs:80-87`).
  With no committee, `stage_envelope` refuses every envelope with
  `SealedCommitteeUnset` (`sealed_exec.rs:132,150`).
- Genesis validation, `CommitteeRecord::validate` (`mod.rs:205`): empty
  committee, threshold outside `1..=members`, duplicate or zero member index,
  and non-canonical points are refused. Index zero is refused because it is
  where the secret sits in the polynomial (`genesis.rs:529-531`).
- Liveness budget: `absentee_budget = members - threshold`
  (`committee.rs:151`).
- Fixed at genesis. Rotation "would mean re-encrypting or abandoning every
  pending envelope" (`mod.rs:81-84`).

## The decrypt trigger

- A member produces a share only after the block fixing the payload's position
  has committed (`cipher.rs:236-241`). Doing it earlier hands the member the
  plaintext while the position is still open, which moves the extraction
  opportunity rather than removing it (`cipher.rs:238-241`).
- The chain enforces this by addressing: a share is keyed to
  `(this block's height, envelope id)`, so a share for any other height finds no
  envelope and fails with `UnknownSealedEnvelope` (`sealed_exec.rs:239-248`).
  Shares are valid only in the block at `reveal_height`
  (`sealed_exec.rs:200-206`).
- At `reveal_height`, `settle_sealed` opens an envelope when at least
  `threshold` stored shares exist (`sealed_exec.rs:416-418`) and the AEAD opens
  under the AAD (`sealed_exec.rs:422-428`).
- If it cannot open (too few shares, bad ciphertext, undecodable plaintext), the
  envelope expires: it is deleted, the nonce stays spent, nothing else moves
  (`sealed_exec.rs:318-325,402-408`; `lib.rs:30-33`).
- No plaintext fallback exists, on purpose. A fallback after a deadline would
  let an attacker who withholds shares wait and read; expiry fails toward
  nothing happening (`lib.rs:35-42`).

## Limits

| Limit | Value | Source |
|---|---|---|
| Envelopes per block | 128 | `crates/node/src/sealed/mod.rs:55` |
| Shares per block | 1,024 (bounds proof verification: two scalar multiplications each) | `mod.rs:62` |
| Minimum reveal delay | 2 blocks | `mod.rs:70` |
| Maximum reveal delay | 256 blocks | `mod.rs:77` |
| Ciphertext size | 48 bytes to 128 KiB | `crates/node/src/core/sealed_payload.rs:34,40` |
| Nesting | a revealed `Seal` or `RevealShare` is dropped | `sealed_exec.rs:434-440` |

Other rules:

- A revealed action that fails is a no-op, not an error. Otherwise anyone could
  seal a guaranteed-failing payload to void the block that opens it. The action
  runs on a copy of the overlay that is committed only on success
  (`sealed_exec.rs:33-43,327-346`).
- The action runs with a nonce taken from the id's low eight bytes, not the
  sender's spent transaction nonce (`sealed_exec.rs:378-398`).
- Sealing is opt-in: a sender who wants guaranteed inclusion sends an ordinary
  transaction (`lib.rs:44-45`).
- Relation to batch settlement: ADR-025 lists the uniform-price DEX batch and
  the sealed mempool as the two in-core MEV protections. The batch covers swaps
  on every network; the sealed mempool covers other transaction kinds only on
  networks with a committee (ADR-025, "Consequences").

## What is not built

- **Distributed key generation.** Setup is a single-party call
  (`committee.rs:77-85`; `lib.rs:47-50`).
- **Committee rotation, by governance or otherwise.** The code says governance
  would put "the confidentiality of the mempool in the hands of a vote" and
  leaves rotation unbuilt rather than built badly (`sealed/mod.rs:81-84`). No
  vote exists, and nothing in the tree changes the committee after genesis.
- **Post-quantum threshold encryption** (`lib.rs:58-60`).
- **Censorship resistance.** A producer can drop a ciphertext (`lib.rs:24-25`).
  ADR-025 records censorship as not provable from one block and out of scope
  for slashing.
- **Slashing for early or withheld shares.** Staking-based slashing does not
  exist (ADR-025, "What validators are forbidden to do"). A bad share is
  rejected and attributed, not punished.
- **A distributed ceremony or external review of any real committee.** Nothing
  in the tree records one; treat it as not done.

## Tests

`features.toml` line 1910 names `crates/mev/tests/encoding_tests.rs`,
`crates/mev/tests/shamir_tests.rs`, `crates/mev/tests/threshold_tests.rs` and
`crates/node/tests/sealed_tests.rs`. This document did not run them.
