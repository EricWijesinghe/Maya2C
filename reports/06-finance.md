# 06 — DeFi, finance, identity and the physical world

**Machine:** cloud VM, 4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4,
`nightly-2026-07-15`. Test lines are from the 2026-09-27 full workspace run
(`--profile ci`) unless marked as run for this report.

## Verdict

DONE WHEN: *every module has tests, a `features.toml` entry with its class,
and registered invariant hooks; this report is complete.*

| Module (brief §) | Tests | Ledger entry | Invariant-guard hook | State |
|---|---|---|---|---|
| DEX / AMM + batch auctions (§1) | `dex_tests.rs` 27 ✓, `crates/dex/tests/*` | yes | yes (AMM reserves, invariants 6–7) | met |
| Payment channels "Maya Flash" (§2) | `channel_tests.rs` 26 ✓, `scale_tests.rs` 4 ✓ (1 ignored: 20k signatures, ~1 h) | yes | yes (channel layer under the state root) | met; machine barter / 10k IoT devices not built |
| Cross-chain (§3) | **new** `crates/btc-spv` 9 ✓; `interop::beacon` (Ethereum sync-committee light client, verified on a real mainnet fixture — `reports/25-interop.md`) | yes | n/a (not on chain) | Bitcoin SPV, **Bitcoin lock/mint and burn/release** (`crates/btc-bridge`, §8 below) and an Ethereum light client; no Ethereum asset route |
| ISO 20022 (§4) | `iso20022_tests.rs` 20 ✓ | yes | n/a (bridge) | met for parsing/generation; types hand-written, not generated from XSDs |
| RWA (§4) | `rwa_tests.rs` 11 ✓ | yes | yes | `ten_thousand_dividends_settle_in_one_block` (the brief's number) and DvP present |
| CBDC / dark pool (§4) | **new** `crates/permissioned-finance`: `vault_tests.rs` 3 ✓, `compliance_tests.rs` 4 ✓ | yes (new, RESEARCH) | n/a (not on chain) | built — §4 below; the MPC form of the dark pool is not |
| Compliance + tax (§5) | sanctions and credential STARK statements in `crates/zk-stark`; **new** `permissioned_finance::tax` | yes (new) | — | US/UK/DE calculators and `compliance_tests.rs` (500 transactions) built; `docs/LEGAL_NOTICE.md` applies |
| Macro-economic engines (§6) | **new** `econ/` 6 ✓ | yes (new, SIM) | n/a (simulation) | built as an agent-based simulator first, as the brief requires |
| Identity (§7) | `identity_tests.rs` 15 ✓ | yes | yes | DID + attestations + selective disclosure; **biometric PoP** (`crates/personhood`, §6 below); EEG/BCI SIM not built |
| DePIN / IoT (§8) | `iot_anchor_tests.rs` 7 ✓ | yes | yes | enrollment → telemetry → tamper; **energy protocol parsers** (`hal/energy`, §5 below); satellite NDVI oracle, swarms not built |
| Oracle (§8) | `oracle_tests.rs` 30 ✓ | yes | yes (invariant 9) | quorum median (one liar cannot move it) and stale-feed refusal present; a dispute window is not |
| Settlement | `settlement_tests.rs` 20 ✓ | yes | yes | — |

## 1. Bitcoin SPV (new)

`cargo test -p maya-btc-spv` — 4 unit + 5 integration tests pass. Highlights:

- **Real mainnet headers 0, 1, 2** hash to the published block hashes, meet
  their targets, and chain (`real_mainnet_headers_hash_meet_their_targets_and_chain`).
- Compact-target encoding matches Bitcoin Core's `arith_uint256` vectors,
  including the negative and overflow encodings Core rejects.
- Retarget: on-schedule keeps the target, the 4× clamp holds both ways, the
  pow limit caps it.
- **6-block reorg:** a deposit 6 deep on the honest chain disappears when an
  attacker publishes 7 blocks from height 4, and returns when the honest chain
  regains the lead. Confirmation depth is a policy the bridge states, not a
  proof.
- Merkle proofs hold at every position for 1–19 transactions and refuse a
  64-byte "transaction" and the duplicate-node shape (CVE-2012-2459).

A real-chain retarget vector was deliberately not included: it would need the
timestamps of blocks 30,240 and 32,255 taken on trust.

## 2. Macro-economic engine (new, SIM)

`econ/` — see `reports/18-economics.md` for the full run. The brief's three
shocks, from a funded starting point (1.5× the break-even price for 100
validators; the draft's own 1.0 input funds only 4):

| Shock | min staking | validators (min) | supply change | stable |
|---|---|---|---|---|
| 30% currency devaluation (costs × 1.43) | 50.3% | 100 (100) | +1.35% | yes |
| 50% market crash | 50.3% | 74 (74) | +1.32% | yes |
| liquidity shock (demand −90% 60 days, 20% unstake) | 50.3% | 102 (100) | +2.11% | yes |

"Stable" = staking ratio ≥ 1/3 and ≥ 2/3 of the initial set solvent on every
day, supply ≤ `MAX_SUPPLY`. **No module guarantees price stability**; prices
are scenario inputs. Real-time ZK proof-of-reserves is not built.

## 3. Not built

Per-order well-formedness proofs and a dealer-free offline phase for the
dark-pool MPC (§9 catches dishonest servers, not dishonest traders); generic L1/L2 light-client framework; IEC 61850
MMS and sampled values; satellite NDVI oracle; robot swarm auctions; EEG/BCI
SIM (built since: §10); liveness and uniqueness for proof-of-personhood; machine-to-machine
barter with 10,000 devices.

## 4. Permissioned finance (new, RESEARCH) — run for this report

Run on the Windows workstation, 2026-09-27, `nightly-2026-07-15`.

`cargo test -p maya-permissioned-finance -- --nocapture`: 7 passed
(`compliance_tests` 4, `vault_tests` 3), 0 failed.

- **CBDC vault** (`cbdc.rs`). Admission verifies a STARK proof
  (`zk-stark::credential`) that some unrevoked credential in the KYC issuer's
  tree carries a tier ≥ the one requested. Tested: a tier-1 holder cannot
  produce a tier-2 proof, a revoked credential cannot be proved, a tier-2 proof
  does not admit at tier 3, and a proof against another issuer's roots is
  refused. Then `fifty_thousand_compliant_settlements_conserve_supply`:
  `50000 settlements in 12.1 ms (4121774/s)` in a debug build, supply equal to
  the sum of balances afterwards. Per-tier limits, freezes and issuer-only
  minting each have a refusal test.
- **Dark pool** (`darkpool.rs`). Orders enter as `BLAKE3(order ‖ salt)`
  commitments; the book is commitments only until the batch closes. Reveals
  must open their commitment; unrevealed orders do not trade. Clearing is at the
  one price that maximises matched volume (lowest on a tie). The 500-order test
  checks the price against a brute-force auction, bought = sold, and that no
  unrevealed trader was filled. `cargo bench --bench darkpool_bench` (release):
  `darkpool 1000 orders (best of 20): commit 0.323 ms, reveal 0.376 ms, clear
  0.129 ms, total 0.828 ms; price Some(100)`.
- **Tax** (`tax.rs`). US (FIFO, short/long split at 365 days), UK (single
  average-cost pool), DE (FIFO, lots held > 365 days exempt), integer cents.
  500 generated trades: US and DE gains sum exactly to net cash; the UK pool is
  within one cent per disposal of it (allowable cost rounds down). A
  hand-worked case pins each jurisdiction. Not tax advice.

**Limits, stated:** `darkpool` is commit-reveal: prices and sizes are
public after the batch closes (`mpc_darkpool`, §7, removes that). The KYC proof is not bound to the account
being admitted: the credential circuit keeps the subject private and exposes
no public input to bind it to, so a proof could be replayed by another account.
Closing that needs a subject-commitment public input in
`zk-stark::credential`, which changes a circuit other statements use; it is
listed here rather than patched around.

## 5. Energy protocols (new, RESEARCH) — 2026-09-28

`hal/energy`, gateway-side; the node links none of it. Windows workstation,
`nightly-2026-07-15`, debug build.

- **Modbus/TCP** (`modbus.rs`): MBAP header and functions 03, 04, 06 and 16,
  plus exception responses. Tested on the worked examples in the Modbus
  Application Protocol specification v1.1b3 (§6.3, §6.6, §6.12, §7). Also
  tested: out-of-range counts, byte counts that disagree, a wrong protocol id
  or MBAP length, a response of the wrong size or table, and every truncated
  prefix of a valid ADU, all refused without a panic.
- **IEC 61850-8-1 GOOSE** (`goose.rs`): APPID header, BER `goosePdu`,
  mandatory fields, and data sets of booleans, integers, unsigneds, 32-bit
  floats, bit strings, strings, times and nested structures. Nesting is capped
  at 8 and BER lengths at four bytes; `numDatSetEntries` must match. The test
  frame is hand-encoded from the standard's ASN.1, **not captured from a
  relay**. MMS and sampled values are not parsed.
- **IEEE 1547-2018** (`ieee1547.rs`): the default frequency trip settings
  (60 Hz) and the Category II voltage defaults, in integer mHz, per-mille and
  ms. An excursion trips only once it has lasted its clearing time, and a
  return inside the band resets it. The values are transcribed, not checked
  against a purchased copy; an interconnection uses its utility's settings.
  IEEE 1547 has no wire format of its own.
- **Market** (`market.rs`, **SIM grid**): the price rises with
  under-frequency and falls with over-frequency, clamped. Green certificates
  are minted once per MWh since enrollment, never twice. A meter reading that
  goes backwards is refused, and a certificate retires once.

```
$ cargo test -p maya-energy
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

A security review of the parsers found no CRITICAL, HIGH or MEDIUM issue.

## 6. Biometric proof-of-personhood (new, RESEARCH) — 2026-09-28

`crates/personhood`, device-side. This is a Juels–Wattenberg fuzzy
commitment: a 128-bit secret is bound to a 2,048-bit iris code by a 15×
repetition code. The secret seeds an ML-DSA-65 key, and **only the public key
is published**; the helper data never leaves the device. To prove presence,
the device scans, corrects the scan to the secret, re-derives the key and
signs the verifier's challenge. Proofs expire after 3 s and a challenge is
accepted once; the verifier forgets challenges once they have expired, and
its clock never runs back.

Measured on synthetic templates with independent bit noise (debug build):

```
bit-flip rate 50/1000: 200/200 genuine scans accepted
bit-flip rate 100/1000: 200/200 genuine scans accepted
bit-flip rate 150/1000: 180/200 genuine scans accepted
bit-flip rate 200/1000: 109/200 genuine scans accepted
test result: ok. 4 passed; 0 failed; 1 ignored
```

Impostor templates never matched (200 of 200 refused). The brief's
100,000 verifications, release build:

```
$ cargo test --release -p maya-personhood --test personhood_tests -- --ignored --nocapture
100000 private verifications in 54.4676899s (scan, correct, derive key, sign, verify); 3 false rejects, each met by a rescan
```

That is 0.54 ms per verification. 3 false rejects at 5% noise matches the
expected rate of about 2 per 100,000. A real iris code's noise is bursty,
not independent, so these rates are not a biometric FRR.

**Not built:**
- **Liveness.** A replayed sensor feed passes; the brief's TEE needs
  attestation that does not exist (invariant 11).
- **Uniqueness.** One person could enrol twice; stopping that means comparing
  templates at enrollment, which this design never sees.
- **The iris extractor itself.**

Repetition-code helper data leaks about the template, which is why it stays
on the device. A security review found a stored hash of the secret would let
whoever takes the helper test template guesses at one hash each; the
recovered secret is now checked by re-deriving the key against the
commitment instead (one ML-DSA key generation per guess). It also found the
replay set grew without bound and was keyed on the nonce alone. Both are
fixed, and the secret is a zeroizing type.

## 7. Dark pool over secret shares (new, RESEARCH) — 2026-09-28

`crates/permissioned-finance/src/mpc_darkpool.rs`. A trader turns an order
into a demand curve and a supply curve over a 256-tick grid and splits each
into additive shares mod 2^64, one per server. A server only adds what it
receives, so no server — and no coalition missing one — sees an order.
Published shares sum to the aggregate curves, which give the uniform price
by the same rule as the open auction. Each trader computes its own pro-rata
fill, and the fills are summed through the same sharing. Rounding goes to a
public residual account, so bought = sold + residual.

```
$ cargo test -p maya-permissioned-finance --test mpc_darkpool_tests -- --nocapture
mpc darkpool 2 orders, 3 servers: price None, volume 0, bought 0, sold 0, residual 0, 176.7µs
mpc darkpool 10 orders, 3 servers: price Some(117), volume 850, bought 848, sold 850, residual -2, 278.3µs
mpc darkpool 500 orders, 3 servers: price Some(119), volume 31627, bought 31564, sold 31627, residual -63, 12.2224ms
mpc darkpool 1000 orders, 3 servers: price Some(121), volume 62116, bought 62116, sold 61993, residual 123, 27.4446ms
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

Price and volume equal `darkpool::clear` at every size tested (debug build;
the timing includes share generation for all three servers).

**What it reveals:**
- the aggregate curves, the price, the volume, the fill totals and the
  residual;
- a lone trader's order, through the aggregate.

A security review found three problems, now fixed and tested:
- **CRITICAL:** aggregates are sums mod 2^64, so two honest orders large
  enough to wrap would have cleared at a wrong price. Orders are now capped
  at 2^40 lots and batches at 2^20 orders, which keeps every sum below 2^60.
- **HIGH:** sharing a fill among fewer than two servers returned the fill
  itself. The splitter now refuses that for every caller.
- **MEDIUM:** aggregates could be read mid-batch, which reveals orders as
  differences between reads. They are published only once every server has
  closed, and servers that saw different batches do not clear.

**Security model:** semi-honest. There are no share MACs and no per-order
proof, so a trader who shares a malformed curve or claims an inflated fill
is not caught. Settlement on chain would reveal fills unless it goes through
the shielded pool, which is not connected.

## 8. Bitcoin lock/mint and burn/release (new, RESEARCH) — 2026-09-28

`crates/btc-bridge`, over `btc-spv`'s header chain. Trust table and depth
policy: [docs/crosschain.md](../docs/crosschain.md).

- **Native validation of external transactions.** Transactions are parsed
  here, legacy and segwit, and txids computed from the non-witness
  serialization. Checked on real mainnet block 900,000 (fetched from
  mempool.space, committed as a fixture): the header hashes to the block's
  id, all 1,562 txids rebuild its Merkle root, and two segwit transactions
  and one legacy transaction parse to their published txids.
- **Lock → mint.** A deposit mints only when its block has at least 6
  confirmations on the most-work chain and a Merkle proof places the
  computed txid under that block's root. It must pay the lock script and
  name exactly one recipient, and each Bitcoin transaction is used once.
- **The brief's six-block reorg.** A deposit five deep is refused. An
  attacker's six-block fork takes it off the best chain, and it is still
  refused. When the honest chain returns, it mints. Nothing is credited
  against a reversed deposit.
- **Burn → release.** A burn opens a release that closes only on a proven
  Bitcoin payment of at least the amount to the named script. The books
  satisfy `supply + owed = locked` after every step.

```
$ cargo test -p maya-btc-bridge
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

An adversarial audit found:
- **CRITICAL:** a release could name the lock script, so one Bitcoin
  transaction both minted and closed a release, creating unbacked wrapped
  BTC;
- **HIGH:** with two `OP_RETURN` recipients, the first one won;
- **MEDIUM:** one transaction could be a deposit and a payout at once.

All three are fixed, and each has a regression test. **Releases still need
whoever holds the lock key**, because Bitcoin cannot check this chain.

## 9. Dark pool against dishonest servers (new, RESEARCH) — 2026-09-28

`crates/permissioned-finance/src/mpc_spdz.rs`: SPDZ-style
information-theoretic MACs (Damgård, Pastro, Smart, Zakarias 2012) over the
prime field 2^61 − 1.
- **Shares and inputs.** Every shared value carries shares of `α·v` under a
  MAC key that no server knows. Traders input through dealer-prepared masks
  and publish only `v − r`, which is uniform.
- **Opening.** An opened aggregate is accepted only after every server
  commits to its MAC residue and then reveals it, and the residues sum to
  zero. The API enforces the order: one commitment per sum, and no reveal
  without every server's commitment. A missing server aborts the check
  rather than shrinking it.

```
$ cargo test -p maya-permissioned-finance --test mpc_spdz_tests -- --nocapture
spdz 1 orders, 3 servers: price None, volume 0, 687.4µs
spdz 10 orders, 3 servers: price Some(119), volume 955, 6.6979ms
spdz 200 orders, 3 servers: price Some(124), volume 13057, 118.8805ms
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.13s
```

Price equals the open auction's at every size. A server that shifts its
share, even while also shifting its MAC share by a guess at the key, is
caught. So is one that reveals something other than it committed to.

A review confirmed the algebra and the soundness of the check, and found
the API permitted misuse. Those findings are fixed and tested:
- **CRITICAL:** a check with no servers passed vacuously;
- **HIGH:** a short residue panicked instead of being refused;
- **HIGH:** a second residue on one sum would reveal a server's key share;
- **HIGH:** commit-then-reveal was not enforced;
- **MEDIUM:** a one-server dealer; a silent `unwrap_or(0)`.

**Still trusted:**
- the dealer, since SPDZ's offline phase (homomorphic encryption or
  oblivious transfer) is not built;
- traders, for well-formed orders, since no per-order proof exists.

Fills stay in the semi-honest `mpc_darkpool` round.

## 10. EEG / BCI authentication simulator (new, SIM) — 2026-09-28

`hal/bci-sim`. Every recording it authenticates in its tests is
synthetic and says so in its EDF header. It has never been run on human
EEG.

- **What is REAL:** the EDF parser (the format clinical EEG uses; tested
  for round-trips and for truncated or inconsistent files). The pipeline
  bins spectral power per channel and frequency into a 2,048-bit template,
  which feeds `maya-personhood`'s fuzzy commitment: an ML-DSA-65 key,
  proofs bound to a challenge, expiring after 3 s, and never accepted
  twice.
- **Spoofing:** the challenge implies a flicker frequency (SSVEP), and a
  live occipital signal must respond at it. A replayed recording responds
  at its own session's frequency and is refused. The check runs on the
  device, so **a device that skips it defeats it**; it needs attested
  hardware, which does not exist here (invariant 11).

```
$ cargo test -p maya-bci-sim -- --nocapture
SIM: synthetic EEG (maya-bci-sim), not a human recording: of 2,048 template bits, sessions of one subject differ in [95, 93, 98, 91, 93, 101]; other subjects in [1045, 1006, 1007, 995, 1019, 983]
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.12s
```

These separations are the generator's, by construction. Whether human EEG
features are this stable and this distinctive is an open research question.

