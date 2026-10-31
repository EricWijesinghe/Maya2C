// The fields Maya2C spans, and the honest status of every part of each one.
//
// Tags follow docs/architecture-vision.md: SHIPPED is in the tree, reachable and
// tested; RESEARCH is in the tree and tested but nothing in consensus calls it;
// SIM is a simulator and says so; PLANNED has no code. The vision document is
// the authority for status, and features.toml is the ledger that backs it.
// scripts/check-fields.mjs fails the build when a tag disagrees with the ledger
// row named by `ledger`, so a status on this page cannot drift from the tree.
export type FieldTag = "SHIPPED" | "RESEARCH" | "SIM" | "PLANNED";

export interface FieldPart {
  ledger: string; // a [[subsystem]] id in features.toml
  tag: FieldTag;
  name: string;
}

export interface Field {
  id: string;
  name: string;
  line: string;
  learn: { href: string; label: string };
  parts: FieldPart[];
  note?: string;
}

export const asOf = "2026-11-01";
export const ledgerFile = "features.toml";

const vision = "/reference/architecture-vision";
const adr = "https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr";

export const fields: Field[] = [
  {
    id: "core",
    name: "Core chain",
    line: "Signatures, state roots, execution and pruning.",
    learn: { href: `${vision}/#2-consensus-and-execution`, label: "Consensus and execution" },
    note: "DAG-BFT finality is not listed. architecture-vision.md tags it RESEARCH and features.toml tags it verified; it appears once the two agree.",
    parts: [
      { ledger: "hybrid-signature-transaction-validation", tag: "SHIPPED", name: "Both signatures checked on every transaction" },
      { ledger: "committed-transaction-root-state-root", tag: "SHIPPED", name: "Committed transaction and state roots" },
      { ledger: "undo-journal-and-reorg-safety", tag: "SHIPPED", name: "Undo journal for safe reorgs" },
      { ledger: "wasm-runtime-via-cranelift-jit", tag: "SHIPPED", name: "WebAssembly contracts" },
      { ledger: "historical-pruning-archive-bootstrap", tag: "SHIPPED", name: "Pruning with archive bootstrap" },
      { ledger: "protocol-spec-conformance", tag: "SHIPPED", name: "Spec and conformance vectors" },
      { ledger: "protocol-upgrade-halt", tag: "SHIPPED", name: "Scheduled protocol upgrades" },
    ],
  },
  {
    id: "mining",
    name: "Mining",
    line: "GPU mining, pools, and the desktop miner.",
    learn: { href: "/guides/mining/", label: "Mining and validators" },
    note: "The block-reward lane (ADR-044) is in progress and is not listed as shipped.",
    parts: [
      { ledger: "gpu-mining-cuda", tag: "SHIPPED", name: "NVIDIA GPU mining (CUDA, off by default)" },
      { ledger: "gpu-mining-wgpu-vulkan-metal-dx12", tag: "SHIPPED", name: "Cross-vendor GPU mining (wgpu)" },
      { ledger: "ethash-style-dag-proof-of-work", tag: "SHIPPED", name: "DAG proof of work" },
      { ledger: "stratum-v2-pool-protocol", tag: "SHIPPED", name: "Stratum V2 pool protocol" },
      { ledger: "maya2c-hub", tag: "PLANNED", name: "Desktop miner app (Hub)" },
    ],
  },
  {
    id: "wallet",
    name: "Wallet",
    line: "A desktop wallet and a command-line wallet.",
    learn: { href: "/guides/wallet/", label: "Wallet integration" },
    parts: [
      { ledger: "tauri-2-0-desktop-wallet", tag: "SHIPPED", name: "Desktop wallet (Tauri)" },
      { ledger: "command-line-wallet", tag: "SHIPPED", name: "Command-line wallet" },
    ],
  },
  {
    id: "chat",
    name: "Chat",
    line: "A post-quantum messenger, detached from the chain.",
    learn: { href: `${adr}/ADR-031-maya-chat.md`, label: "ADR-031: Maya Chat" },
    parts: [
      { ledger: "maya-chat", tag: "RESEARCH", name: "Maya Chat messenger" },
      { ledger: "chat-stickers", tag: "RESEARCH", name: "Stickers" },
    ],
  },
  {
    id: "ai",
    name: "AI",
    line: "Private training research, and AI-checked work.",
    learn: { href: `${vision}/#4-ai-and-hardware-integration`, label: "AI and hardware integration" },
    parts: [
      { ledger: "confidential-federated-training", tag: "RESEARCH", name: "Confidential federated training" },
      { ledger: "lattice-proof-of-useful-work-svp-solver", tag: "RESEARCH", name: "Lattice proof of useful work" },
      { ledger: "zkml-proof-verification-halo2-bn254", tag: "PLANNED", name: "ML proof verification (zkML)" },
    ],
  },
  {
    id: "identity",
    name: "Identity",
    line: "Self-sovereign identities and selective disclosure.",
    learn: { href: `${vision}/#1-cryptography-and-key-management`, label: "Cryptography and key management" },
    parts: [
      { ledger: "self-sovereign-identity-records", tag: "RESEARCH", name: "Identity records (did:maya2c)" },
      { ledger: "did-key-rotation-and-revocation", tag: "RESEARCH", name: "Key rotation and revocation" },
      { ledger: "selective-disclosure-credentials", tag: "RESEARCH", name: "Selective-disclosure credentials" },
      { ledger: "biometric-proof-of-personhood", tag: "RESEARCH", name: "Proof of personhood (biometric)" },
    ],
  },
  {
    id: "governance",
    name: "Governance",
    line: "Proposals with bounds, a council that can pause, and a treasury.",
    learn: { href: `${vision}/#2-consensus-and-execution`, label: "Governance lifecycle" },
    parts: [
      { ledger: "governance-lifecycle-and-bounds", tag: "SHIPPED", name: "Proposal lifecycle and bounds" },
      { ledger: "security-council-pause", tag: "SHIPPED", name: "Security council pause" },
      { ledger: "treasury-vesting-rules", tag: "RESEARCH", name: "Treasury and vesting rules" },
    ],
  },
  {
    id: "telecom",
    name: "Telecom and space",
    line: "Off-grid radio, satellites, and timing.",
    learn: { href: `${vision}/#3-kernel-networking-and-transport`, label: "Kernel, networking and transport" },
    note: "Link models are SIM: they describe radio and satellite links and never carry chain traffic.",
    parts: [
      { ledger: "lora-off-grid-transport", tag: "RESEARCH", name: "LoRa off-grid headers" },
      { ledger: "store-and-forward-mesh-relay", tag: "RESEARCH", name: "Store-and-forward mesh relay" },
      { ledger: "fountain-coded-fragmentation", tag: "RESEARCH", name: "Fountain-coded fragments" },
      { ledger: "timing-service", tag: "RESEARCH", name: "Multi-source timing" },
      { ledger: "link-sim", tag: "SIM", name: "Link models (simulated)" },
      { ledger: "satellite-uplink", tag: "PLANNED", name: "Satellite uplink" },
      { ledger: "leo-free-space-laser-mesh", tag: "PLANNED", name: "Laser mesh between satellites" },
      { ledger: "ccsds-delay-tolerant-networking-bpv7", tag: "PLANNED", name: "Delay-tolerant networking (BPv7)" },
    ],
  },
  {
    id: "hardware",
    name: "Hardware",
    line: "Device identity, energy gear and robots.",
    learn: { href: `${vision}/#4-ai-and-hardware-integration`, label: "AI and hardware integration" },
    parts: [
      { ledger: "iot-anchor-puf-tpm-sealed-device-identity-was-tp", tag: "RESEARCH", name: "Device identity (PUF and TPM)" },
      { ledger: "energy-protocols-markets", tag: "RESEARCH", name: "Energy protocols (Modbus, IEC 61850)" },
      { ledger: "robot-swarm-coordination", tag: "RESEARCH", name: "Robot swarm coordination" },
      { ledger: "photonic-optical-tensor-driver", tag: "PLANNED", name: "Photonic tensor driver" },
      { ledger: "magneto-optical-mram-driver", tag: "PLANNED", name: "MRAM driver" },
    ],
  },
  {
    id: "bio",
    name: "Bio data sovereignty",
    line: "Biology as entropy and archive: simulated or planned.",
    learn: { href: `${vision}/#4-ai-and-hardware-integration`, label: "AI and hardware integration" },
    note: "None of these parts runs on a production path.",
    parts: [
      { ledger: "eeg-bci-authentication-sim", tag: "SIM", name: "EEG authentication (simulator)" },
      { ledger: "biomolecular-trng", tag: "PLANNED", name: "Biomolecular random source" },
      { ledger: "dna-quaternary-archival-engine", tag: "PLANNED", name: "DNA archival engine" },
      { ledger: "bio-silicon-neural-organoid-mea-interface", tag: "PLANNED", name: "Organoid interface" },
    ],
  },
  {
    id: "quantum",
    name: "Quantum harbor",
    line: "Post-quantum signatures, vault accounts, and an exposure check.",
    learn: { href: "/guides/signatures/", label: "Post-quantum signatures" },
    parts: [
      { ledger: "ml-dsa-65-fips-204", tag: "SHIPPED", name: "ML-DSA-65 signatures (FIPS 204)" },
      { ledger: "slh-dsa-fips-205", tag: "SHIPPED", name: "SLH-DSA backup signatures (FIPS 205)" },
      { ledger: "vault-accounts", tag: "SHIPPED", name: "Vault accounts with delay and guardians" },
      { ledger: "quantum-exposure-tool", tag: "SHIPPED", name: "Quantum exposure checker" },
      { ledger: "physical-qkd-km-api-interface", tag: "PLANNED", name: "QKD key-management interface" },
    ],
  },
];
