---
title: 'Can a Ledger sign a Maya2C transaction?'
editUrl: false
# GENERATED from docs/ledger-feasibility.md by scripts/ingest.mjs. Edit the source, not this.
---
**Yes, for suite `0x10` (ML-DSA-65 alone), on Nano S Plus and Nano X under
Speculos, with a low-memory ML-DSA-65 written for the purpose. The hybrid
(`0x30`) still does not fit.**

This records what was built, what was measured, and what was not, so the
conclusion can be checked rather than taken on trust.

## Why suite `0x10`, and what it costs

The chain's legacy signature is a hybrid, ML-DSA-65 + SLH-DSA-SHA2-128s, and
its SLH-DSA half overflowed a 1 MiB desktop stack in this repository. ADR-007's
suite envelope (wire v7) lets a transaction name a single suite, and `0x10` is
ML-DSA-65 on its own: a complete signature, not half of one.

v7 verifies from genesis (ADR-013, 2026-09-27), so a device signature is a
transaction consensus accepts today. Until that ADR the envelope was dark and
this paragraph said so.

## RAM: the measurement that decided it

**App SRAM, from the SDK's link scripts**
(`ledger_secure_sdk_sys-1.16.4/devices/*/<device>_layout.ld`). Statics, heap
and stack share it:

| Device | SRAM | Stack after `.bss` (as built) |
|---|---|---|
| Nano S Plus | 40 KiB | 32,180 B |
| Stax, Flex | 36 KiB | 26,980 B |
| Nano X | 28 KiB | 19,872 B with the default 8 KiB heap; **26,016 B** with `HEAP_SIZE = "nanox: 2048"` |

The brief's fallback, "if it cannot fit on Nano S Plus, target Stax/Flex",
rests on a false premise: Stax and Flex have *less* app RAM than a Nano S Plus.

**Stock `fips204` does not fit any of them.** On a Linux host, the smallest
thread stack each operation completes on, less a 142 KiB baseline for a thread
that does nothing (`tests/memory_tests.rs`, 1 KiB resolution, release build):

| Operation | `fips204` 0.4.6 | `lowmem` (this crate) |
|---|---|---|
| keygen | ~143 KiB | within the 1–2 KiB noise of the baseline |
| sign | ~158 KiB | within the 1–2 KiB noise of the baseline |
| verify | ~43 KiB | — |

**`lowmem` on the device CPU, from the compiler.** These are
`-Z emit-stack-sizes` frames of the linked `nanosplus` ELF, read with
`llvm-readobj --stack-sizes`. The raw table is
`reports/ledger/stack-sizes-thumbv8m.txt`. The deepest signing chain is:

`device::run` 8,176 → `sign_transaction` 6,584 → `sign_into` 1,160 →
`attempt` 4,856 → `w_row` → `accumulate_a_times` 1,208 → `finalize_xof` 440 →
`keccak::p1600` 488. That is **about 23 KiB**, against 32 KiB available on
Nano S Plus.

The first device build put every handler inline in `run`: one 28,008-byte
frame. The stack overflowed on the first `GET_PUBLIC_KEY`. Speculos showed it
as the app exiting. Separating the handlers (`#[inline(never)]`) and writing
keys and signatures into caller-owned buffers fixed it.

## How the low-memory signer works, and why it is trusted

`src/lowmem/` computes FIPS 204 KeyGen and Sign while holding a few 1 KiB
polynomials instead of `fips204`'s full matrix and vectors:

- `Â` is never stored. Each entry is sampled from SHAKE128 and multiplied
  into an accumulator as it arrives.
- `y`, `s1` and `s2` are regenerated from their seeds when needed.
- `w = Â·y` is produced one row at a time. Each row's `w1` is absorbed straight
  into the challenge hash, and the row is recomputed for the hints.

It is trusted because it is not a new function. It equals **NIST ACVP**
ML-DSA-65 keyGen (10 cases) and sigGen (20 cases: deterministic and hedged,
internal and external interface), and it equals **`fips204` byte for byte**
over 64 random keys and messages (`tests/lowmem_tests.rs`). The price is time:
each attempt runs `ExpandMask` and the NTT k·ℓ times instead of ℓ.

## Byte-identical to the wallet

| Step | Device | Pinned by |
|---|---|---|
| Chain key | SLIP-0010 ed25519 at `m/44'/7331'/a'/0'/i'`, the `HDW_ED25519_SLIP10` syscall | `tests/ledger_tests.rs` under Speculos, against an independent host BIP-39 → SLIP-0010 derivation (itself checked against the published vectors) |
| Key | `ξ = BLAKE3-derive-key("…suite 0x10 ml-dsa-65 xi v1", chain key)`, then KeyGen | `tests/parity_tests.rs`, against a fixture the node writes (`crates/node/tests/ledger_fixture_tests.rs`) |
| Address | the node's `suite_address` | same fixture |
| Signature | deterministic, `rnd = 0³²`, empty context | same fixture: the device signs the node's own v7 transfer to the node's own bytes |

## The protocol

| INS | Command | Behaviour |
|---|---|---|
| `02` | `GET_PUBLIC_KEY` | path → page 0 of the 1,952-byte key |
| `06` | `DISPLAY_ADDRESS` | shows the address and waits for approval; returns it only once confirmed |
| `04` | `SIGN_TRANSACTION` | chunk 0 is the path, then the v7 signing bytes (≤ 4 KiB) → review → page 0 of the 3,309-byte signature |
| `08` | `GET_PAGE` | page P2 of the last key or signature |

The review (`src/review.rs`) refuses before any screen appears if the bytes:

- are not under the v7 signing domain;
- are for another suite, or for any key other than this device's;
- are a payload kind rather than a plain transfer;
- have more than 4 outputs or 32 inputs.

What is shown is every output (amount and recipient), the paying account and
the nonce.

## Speculos: what ran

`scripts/ledger_speculos.sh` builds the app and starts Speculos with a fixed
BIP-39 phrase. It then runs `tests/ledger_tests.rs`, driving the buttons
through Speculos's REST API.

- Nano S Plus, API level 27: **8 of 8 pass**.
- Nano X (`DEVICE=nanox`), API level 27: **8 of 8 pass**.

Stax and Flex build (74.8 KB of code each); their touch-screen flows are not
driven by these tests.

Environment, set up in WSL Ubuntu 24.04:

- `gcc-arm-none-eabi`, `clang`, `qemu-user-static`;
- Speculos 0.27.0 and ledgerblue, in a venv;
- `cargo-ledger` 1.14.0;
- `ledger-secure-sdk` at branch `API_LEVEL_27`. Speculos refuses `master`,
  which declares API level 0.

Two build fixes were needed:

- `blake3` is pinned to 1.8.2, because 1.8.7's build script panics on a
  single-word target name.
- `-Zbuild-std` goes on the command line, not in `.cargo/config.toml`, where
  it would break host `cargo test`.

## Review findings

Both reviewers ran over this change. Fixed here:

- **Key-dependent intermediates were not zeroized.** Only the top-level secret
  key was. `ρ'`, `ρ''` and the polynomial scratch buffers hold `s1`, `s2`, `t0`
  and the mask `y` in the clear, and any of those recovers the key. All are
  `Zeroizing` now, which also covers the rejection loop's early returns. The
  attack this closes is physical — SRAM read off a seized device — which is
  what a hardware wallet exists to resist.
- **The derivation syscall's failure was invisible.** It returns `void` and
  signals failure by throwing, so a Rust caller sees only the buffer. An
  all-zero result is now refused with `0x6F01` rather than signing under a key
  the recovery phrase does not control.
- **An amount could lose its unit label on screen.** The field buffer was 24
  bytes; 20 digits plus " base units" is 31. It is 40 now, with a compile-time
  assertion, because a review screen must not truncate silently.
- **Two silent zero fallbacks** in `keygen` became `expect`: a broken invariant
  there would have minted a working-looking wrong key.

Reported and not changed:

- **"The review cannot show the fee, so an invisible fee could drain the
  account."** That is true of a UTXO chain and not of this one. A transfer
  debits the sender exactly the sum of its outputs, and there is no fee field
  (`crates/node/src/state/db.rs`: `total_outputs`, then `debit`). A
  transaction's `inputs` are encoded and read by no state transition, so the
  outputs on screen are the whole debit.
- **Addresses are shown as 64 hex characters**, which is easy to skim rather
  than compare. There is no shorter checksummed form for a suite-`0x10`
  address yet; inventing one here would be a new address format nothing else
  in the tree understands.

## What was not measured

- **Signing time on hardware.** Speculos does not emulate device timing. The
  full suite, UI included, runs in about 17 s on this machine, which says
  nothing about a real secure element.
- **Real hardware.** No device was used.
- **The SDK's own ML-DSA** (`ledger_device_sdk::mldsa`, Ledger's C
  `lib_cxng`). Its keygen draws internal randomness and takes no seed, so its
  keys cannot be re-derived from the recovery phrase. Its signing is not
  documented as deterministic, so its signatures would not match the wallet's
  transaction ids. It was read, not used.
