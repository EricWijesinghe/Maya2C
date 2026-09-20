# Can a Ledger sign a Maya2C transaction?

**Not today, and the obstacle is the hash-based half.**

This records what was built, what was measured, and — importantly — what was
not, so the conclusion can be checked rather than taken on trust.

## The constraint the brief did not state

A Maya2C signature is a **hybrid pair**, and `HybridVerifyingKey::verify`
(`crates/node/src/crypto/hybrid.rs:338-340`) checks both halves:

| Half | Scheme | Bytes |
|---|---|---|
| lattice | ML-DSA-65, FIPS 204 | 3,309 |
| hash-based | SLH-DSA-SHA2-128s, FIPS 205 | 7,856 |
| | **signature** | **11,165** |
| | public key | 1,984 |

A device that produces only the lattice half has produced nothing the chain
accepts. "Implement `SIGN_TRANSACTION (ML-DSA-65)`" is therefore not a smaller
version of the job — it is a device that cannot make a valid transaction.

## What was measured

| Fact | Value | How |
|---|---|---|
| Protocol layer + `fips204` compile for Cortex-M | **yes** | `cargo check --target thumbv8m.main-none-eabi --lib`, clean |
| `fips204` ARM release rlib | 493 KiB | built here |
| `blake3` ARM release rlib | 185 KiB | built here |
| SLH-DSA keygen peak stack, x86-64 | **> 1 MiB** | it overflowed the default main-thread stack in this repo; `crates/node/src/bin/genesis-ceremony.rs` now runs on a 16 MiB thread because of it |
| Hybrid signature over a 255-byte APDU | 44 responses | `crates/node/tests/apdu_tests.rs`, asserted |
| Hybrid public key over a 255-byte APDU | 8 responses | same |

`fips204` is `#![no_std]` with **no heap allocations** and ships an embedded
example. The lattice half is the plausible one.

`slh-dsa`'s own documentation (`slh-dsa-0.2.0-rc.5/src/lib.rs:20`) says it
"allocates signatures and intermediate values on the stack, which may cause
problems for environments with limited stack space". The 1 MiB overflow above is
that warning being right.

## What was *not* measured, and why

**Peak stack on ARM, per function.** `-Z emit-stack-sizes` was enabled and
`llvm-readobj --stack-sizes` run over the ARM rlibs. It reports two 32-byte
frames and nothing else, because ML-DSA-65's code is generic and is not
monomorphised until a **final link** — and linking a device binary needs
Ledger's C SDK, which is not present here.

So the decisive number for a device is not in this document. What replaces it is
an order-of-magnitude argument: a routine that overflows 1 MiB on x86-64 is not
going to fit in a budget measured in kilobytes, and the ARM frames would have to
be ~100× smaller for the conclusion to change.

**Anything about real hardware.** No device, no Speculos, no
`arm-none-eabi-gcc`. `apps/ledger-maya2c/tests/ledger_tests.rs` is written and every
test in it is `#[ignore]`.

## Where the device build stops

`cargo check --target thumbv8m.main-none-eabi` on the **binary** fails in the
SDK's build script:

```
thread 'main' panicked at ledger_secure_sdk_sys-1.16.4/build.rs:252:
Unsupported target_os: none
```

That is not a bug in the app. Ledger does not build against a stock triple — it
ships its own target JSONs whose `target_os` is the device name. From that build
script's `SPECS`:

| Device | `target_os` | Triple | SDK env var |
|---|---|---|---|
| Nano X | `nanox` | `thumbv6m-none-eabi` | `NANOX_SDK` |
| Nano S Plus | `nanosplus` | `thumbv8m.main-none-eabi` | `NANOSP_SDK` |
| Stax | `stax` | `thumbv8m.main-none-eabi` | `STAX_SDK` |
| Flex | `flex` | `thumbv8m.main-none-eabi` | `FLEX_SDK` |

`app-maya2c` gates the SDK dependency on exactly those four values, so
`--target thumbv8m.main-none-eabi --lib` checks the protocol layer for ARM
without needing the C SDK at all. That is how the "compiles for Cortex-M" row
above was obtained.

To go further you need a Ledger C SDK checkout, `LEDGER_SDK_PATH` (or the
per-device variable), an ARM C toolchain, and the device target JSON.

## What exists

```
apps/ledger-maya2c/
  crates/node/src/apdu.rs     framing, chunk assembly, response paging   20 tests, host
  crates/node/src/derive.rs   path validation, address derivation        pinned to the node
  crates/node/src/sign.rs     ML-DSA-65 keygen and signing from a seed
  crates/node/src/main.rs     device shell; SIGN_TRANSACTION returns 0x6A81
  crates/node/tests/apdu_tests.rs      20 passing
  crates/node/tests/ledger_tests.rs    4, all #[ignore]
crates/node/tests/ledger_parity_tests.rs   4 passing, in the node's suite
```

`SIGN_TRANSACTION` refuses rather than returning a lattice-only signature. A
device that emitted 3,309 bytes and called it done would look like it worked and
would produce transactions every node rejects.

### A bug the parity test exists for

The app's first address derivation used
`blake3::Hasher::new_derive_key(ADDRESS_DOMAIN)`. The node prefixes the domain
into a **plain** hasher (`crates/node/src/crypto/hybrid.rs:216`). Those produce different
digests from identical input, and the failure mode is an address no key can
spend from, reported by nothing.

It was caught by reading the node rather than by a test. `crates/node/tests/ledger_parity_tests.rs`
now pins the derivation against `address_of` over freshly generated keys, and
separately asserts that the keyed form does *not* match — so a future "tidy-up"
back to `new_derive_key` fails loudly.

## Options, if a Ledger is wanted

1. **Reduce the parameter set.** SLH-DSA-SHA2-128**s** is the small-signature,
   slow-signing variant. The `f` variants sign faster with larger signatures;
   neither obviously fits. This is a consensus change and affects every
   signature on the chain.
2. **Split custody.** Device holds the lattice key, host holds the hash-based
   one. Produces valid transactions and **ends the device's value** — a
   compromised host forges the half the device does not hold.
3. **Wait for the ecosystem.** Ledger's secure element gains PQ primitives, or a
   stack-optimised SLH-DSA appears. Neither is in hand.
4. **Accept that this chain's signature does not fit a Ledger** and treat the
   air-gapped QR flow in `apps/wallet-gui/core/src/airgap.rs` as the offline story.
   It already exists, already works, and already handles the 11,165-byte
   signature — in 512 frames if it must.

Option 4 is the honest default. The others are decisions somebody makes
deliberately, with this document in front of them.
