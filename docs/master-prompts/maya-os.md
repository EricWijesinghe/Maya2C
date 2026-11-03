# Maya OS and our own hardware brand

Status: **PLANNED**. Planning only: no code, no crate, no `Cargo.toml` change, no money.
Band: **P6** (deferred and frontier, untouched until its milestone, CLAUDE.md "Choosing what to do next").
Start: after mainnet, and with Eric's go (CLAUDE.md, "Forbidden without Eric's instruction": new frontier modules).
Gate: each phase below needs an ADR and a threat model before code (owner-directive 2026-10-17, quoted in section 1).

Companion: [next-evolution-gap.md](next-evolution-gap.md) (the brief's gap table; Maya OS is not in the brief).

## 1. Vision

Memory records no verbatim statement from Eric that names Maya OS or the hardware brand. The text below is what the memory holds, with the source file for each. Paraphrases are marked.

| Date | Source | Text |
|---|---|---|
| 2026-10-17 | `owner-directive-2026-10-17.md` | Record of the order, not a quote: "decentralized comms → storage → app hosting → decentralized AI → on-chain representative democracy → Maya OS + own hardware brand". Each later phase needs "ADR + threat model first". |
| 2026-10-17 | same | Quote: "max security". Quote: "everything is pre authorized", scoped to testnet operations, tools and the Phase 1 security plan. It does not cover money, mainnet genesis or economics. |
| 2026-10-12 | `platform-compliance-directive.md` | Quote: "when everything is launched we should be well prepared not to flag our programs from OS or any other security by being maximum supportive to their systems regulations while obtaining all the resources needed legally from system security." |
| 2026-10-12 | same (follow-up, same day) | Paraphrase: being outside flags must not cost power, efficiency or functionality. Compliance comes from signing, disclosure, consent and standard APIs, never from throttling or a weaker build. |
| 2026-09-28 | `owner-vision-utopia.md` | Paraphrase: "Orville-style utopia" as the long-term aim. Each app is a separate, detached binary, "never one super-app". |
| 2026-10-11 | `field-extensions-plan.md` | Quote: "a financial giant, not a baby ecosystem". Paraphrase: each field is a detached app on one platform layer. |
| 2026-10-12 | `platform-targets-2026-10-12.md` | Quote: "Till then keep finishing things." Paraphrase: macOS and iOS wait for hardware; keep code portable. |

Constraints that bind this plan (memory): the owner is the sole Rust developer and keeps non-Rust code minimal (`owner-vision-utopia.md`). Claude never holds keys, signs for legal matters, or deploys without `APPROVED: <step name>` (`claude-role-boundaries.md`).

**Open for Eric:** an OS and a hardware brand are a different kind of product from a chain. He should state in his own words what "Maya OS" must do for a user, before the ADR in step 1 (section 6).

## 2. What exists that Maya OS would build on

Status from `features.toml` unless marked otherwise. Paths are repo-relative.

| Piece | Path | Status | Use for Maya OS |
|---|---|---|---|
| Entropy HAL: OS and RDSEED sources, SP 800-90B health tests, SP 800-90A DRBG | `hal/entropy` | REAL verified (`entropy-hal`); **RESEARCH** in `docs/architecture-vision.md` §1 | Device RNG. The two documents disagree; see section 7. |
| IoT anchor: PUF or TPM-sealed device identity, ML-DSA-65 device keys, TPM seals and never signs | `hal/iot-anchor`, `crates/node/src/state/iot_exec.rs` | RESEARCH working | Device identity pattern. Enrolment proves key possession, not genuine silicon. |
| MCU firmware, `no_std`, measured under QEMU (Cortex-M33) | `hal/iot-firmware` | RESEARCH | Base for sensor or gateway-class devices. |
| Linux XDP and AF_XDP relay (aya, Linux only) | `hal/ebpf-net` | RESEARCH working | Kernel-side networking on a Linux base image. |
| LoRa, fountain coding, store-and-forward relay | `hal/radio-transport` | RESEARCH working | Off-grid transport. Radio is regulated (section 4). |
| Energy, swarm, BCI simulator, link simulator | `hal/energy`, `hal/swarm`, `hal/bci-sim`, `hal/link-sim` | RESEARCH / SIM | Application hardware. Not OS prerequisites. |
| GPU miners: CUDA (default-off, invariant 3), wgpu (Vulkan, Metal, DX12) | `hal/cuda-miner`, `hal/wgpu-miner` | REAL verified | Drivers only through standard APIs (platform rule). |
| Sealed validator keys at rest | `crates/node` (ADR-049) | REAL working | Key-at-rest model for the image. |
| Remote validator signer with slashing protection | `crates/signer` | REAL working | A separate signing device pattern. |
| Offline (air-gapped) signing loop | `offline-signing-loop` in `features.toml` | REAL verified | Keys that never touch the networked OS. |
| Reproducible Linux node builds | `docs/adr/ADR-054-reproducible-builds.md`, `release-binaries.yml` | Decided | Base for a bit-identical image. |
| In-app updater, release builds only, checked against a compiled-in public key | `apps/app-update` | Present; signature enforcement not verified here | The update channel. Needs its own test before reliance. |
| Desktop shells: Hub, Wallet, Chat | `apps/hub`, `apps/wallet-gui`, `apps/chat` | Hub PLANNED (`maya2c-hub`); Wallet REAL; Chat RESEARCH | Surfaces the OS would boot into. Mobile targets are scaffolded only (`cfg(mobile)`). |

**Not in the tree** (checked by search, repo-wide): secure boot (no hits), measured boot, TEE or vendor attestation (`docs/architecture-vision.md` §4: "no SGX / SEV-SNP report is generated or verified yet"), an OS image build, hardware design, a device fleet, certification.

## 3. Phases

Each phase exits on evidence, not on a demo. Order follows section 1.

| Phase | Goal | Exit evidence | Depends on |
|---|---|---|---|
| A. Scope | ADR, threat model, prior-art note | ADR accepted; `docs/prior-art/maya-os.md` with a dated search | Eric's statement (section 1) |
| B. Image | Reproducible Linux base with the node and Hub as the shell; no kernel modules | Two independent rebuilds are bit-identical; an update with a bad signature is refused by test | ADR-054, `apps/app-update`, ADR-049 |
| C. Boot | Secure boot with our keys on a reference PC platform; TPM-sealed key release (TPM seals, never signs) | Tampered boot image refused, shown in a test log | Phase B; key ceremony (section 4) |
| D. Identity | Device enrolment and attestation under the model chosen in section 4 | Enrolment test; attestation claims labelled with what they prove | Phase C; invariant 11 decision |
| E. Reference hardware | Board, secure element, radio certification path | Board boots Phase B image; certification plan with quoted costs | Phase D; money (Standing Order 6) |
| F. Fleet and brand | Enrolment, updates, support, warranty | Fleet runs a staged update and a rollback | Phase E; legal advice; Eric's decision |

Placement in the roadmap: Maya OS comes after governance (utopia Phase 6) and the fields (Phase 7) in the owner's order. Phases A and B could start earlier if Eric reorders them.

## 4. Hard dependencies and risks

**Attestation model versus invariant 11.** `docs/invariants.md` #11: the oracle is optional and absent by default, because introducing a trusted party is a decision somebody writes down. A TEE or vendor quote is trusted-party-shaped (`docs/architecture-vision.md` §4 calls it "invariant-11-shaped"). CLAUDE.md lists "TEE attestation (11)" among the PLANNED items that must be reconciled before code. This plan does not resolve it.

| Option | What it trusts | Invariant 11 | Verdict |
|---|---|---|---|
| A. Self-enrolment: device ML-DSA key, owner binding | Key possession only | No collision | Default. Labelled "proves possession, not genuine silicon". |
| B. Vendor-rooted TPM or TEE quote | A vendor root key | Collides; needs a written decision | ADR and genesis opt-in only. Off by default. |
| C. Witnessed attestation (peers vouch) | Peers | Sybil-prone without personhood | Later. Depends on `biometric-proof-of-personhood` (RESEARCH; no liveness, no uniqueness). |

Biometric personhood's liveness also needs a TEE (`docs/architecture-vision.md` §4), so option B decides both.

**Secure boot.** No code exists. The signing keys are secrets (Standing Order 5): typed, zeroized, held offline. Losing them bricks every installed device. A key ceremony needs a witness, and the owner has said no third party before mainnet (`owner-directive-no-third-party-2026-10-28.md`); a post-mainnet ceremony still needs his decision.

**Drivers and GPU.** Vendor drivers are large and not ours to audit. The platform rule allows only standard APIs: wgpu, Vulkan, DX12, Metal. No admin or kernel drivers for mining (`platform-compliance-directive.md`). An OS that ships its own kernel modules for mining would break that rule.

**`unsafe`.** Kernel-facing and driver code needs `unsafe`. CLAUDE.md Execution Directive 4 exempts `hal/` but requires a hardware-attestation justification comment and a reviewer for every block. The directive also says "Holds today" for core paths only; an OS layer is not core.

**Radio and certification.** LoRa and any RF product need equipment authorization per country (FCC, ISED, CE and others). `field-extensions-plan.md` already flags radio licensing. Lab costs are MISSING until quoted; a third-party lab is money and a third party, so it needs `APPROVED`.

**Rust only.** Bootloaders, ROM code and vendor SDKs are often C. Keep any C minimal and vendored with a pinned version, and ask Eric before adding one (memory: owner keeps non-Rust code minimal).

**Consensus-adjacent.** Device records sit under the state root (invariant 25). Device telemetry is flagged, not refused (`iot-anchor`), so a device can't be used to halt a block.

**Money.** Hardware bill of materials, certification, warranty, support: all MISSING. None is quoted here.

**Single maintainer.** Memory records this as a named risk (`utopia-roadmap-phases.md`). An OS adds surface a single person must maintain.

**Apple hardware.** The Phase B image is Linux. macOS and iOS hardware is outside this plan (`platform-targets-2026-10-12.md`).

**ADR-069 conflict.** Feature 7 ("phone-mined first fees") puts mining on phones. That collides with the store rules in section 5 for any app-store build. It must be resolved in ADR-069 before any mobile share lane is planned.

## 5. What must never happen

- Phone mining in an app-store build, or any mining that is hidden, persistent without consent, or started without an explicit user action. Desktop mining is visible and opt-in (`platform-compliance-directive.md`).
- Kernel or admin drivers for mining; any feature that hides from, evades, obfuscates or bypasses OS or AV security tools.
- Lowering hash rate, dropping features or shipping a weaker miner to avoid a flag. Compliance is signing, disclosure, consent and standard APIs.
- Unsigned updates, or an update path that skips the signature check. No update channel without a verifiable key.
- Keys or signing material in logs, images, or the repository (Standing Order 5).
- Vendor attestation enabled by default. Invariant 11 needs a written decision first.
- Claims of "first", "only", "unprecedented", or a shipping device before it is built and run (Standing Order 10; `docs/prior-art/<feature>.md` must record a dated search).
- Any hardware order, certification lab, or signing ceremony without `APPROVED: <step name>`.

## 6. Steps 1-3 after mainnet

In order. Each needs Eric's go.

1. **Scope ADR and threat model.** Write `docs/adr/ADR-0NN-maya-os.md` (next free number; ADR-076 is the renumbered base-fee ADR). Cover the attestation options in section 4 and reconcile invariant 11 in words, not code. Run `security-architect` on the threat model. Output: an accepted ADR and `docs/prior-art/maya-os.md` with a dated search. Eric decides the attestation option.
2. **Image spike on existing hardware.** No purchase. On Eric's PC or a VM on E:, build a Linux base with the node and Hub as the shell, using ADR-054 reproducible builds. Test two things: a second independent build is bit-identical, and an update with a bad signature is refused. Output: test logs in `reports/`. No kernel modules.
3. **Hardware decision memo.** Candidate boards, secure elements and radio paths, with every cost MISSING until quoted. Eric chooses the budget. Nothing is bought, certified or announced. Output: a memo in `docs/master-prompts/` or `reports/`.

Add a `features.toml` entry only when code lands in `Cargo.toml` (CLAUDE.md Workflow Rule 5).

## 7. Discrepancies found while writing this

- `entropy-hal`: `features.toml` says REAL verified; `docs/architecture-vision.md` §1 says RESEARCH. The document is the authority for status (`features.toml` header), so the entry needs reconciling.
- `docs/adr/`: two files were numbered ADR-071 (resolved 2026-11-01: the base-fee ADR is now `ADR-076-base-fee-step-clamp-and-ceiling.md`; `ADR-071-site-integrity.md`).
- Invariant 20 is retired (ONNX, `docs/invariants.md` #20), yet CLAUDE.md Standing Order 4 and Execution Directive 2 cite it for "no floats in consensus". The float reasoning sits inside the retired text. The rule needs its own number.
- Eric's economics memory has changed twice: fees-only (2026-10-04) and block rewards with no premine (2026-10-11, ADR-044). Confirm which holds before any tokenomics ADR.

## 8. Architecture-vision row

`docs/architecture-vision.md` §4 gets one PLANNED row for Maya OS that links here, per Workflow Rule 5. No status above PLANNED.
