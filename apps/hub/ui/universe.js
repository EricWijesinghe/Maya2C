// Every app and field of the Maya2C ecosystem, with an honest status.
// "live" means running on the public testnet today; nothing is labelled live
// because it is planned. Update a status here when it changes in features.toml.
"use strict";

const UNIVERSE = [
  { id: "wallet", name: "Wallet", icon: "◈", status: "live", text: "Post-quantum desktop wallet: send, receive, and keys kept in the OS keychain." },
  { id: "explorer", name: "Explorer", icon: "⌕", status: "live", text: "Every block, transaction and account on the testnet, at explorer.maya2c.dev." },
  { id: "node", name: "Validators", icon: "⬡", status: "live", text: "DAG-BFT committee producing and finalizing blocks. Anyone can run a node." },
  { id: "mining", name: "Mining", icon: "⛏", status: "building", text: "GPU and CPU miners earn from the block-reward reserve. Testnet trial from mid-October." },
  { id: "chat", name: "Maya Chat", icon: "✉", status: "building", text: "Encrypted peer-to-peer chat. Messages wait with the sender until delivered, plus animated stickers." },
  { id: "dex", name: "Exchange", icon: "⇄", status: "building", text: "On-chain order book for swapping assets." },
  { id: "identity", name: "Identity", icon: "◉", status: "building", text: "One identity across every app, owned by you." },
  { id: "governance", name: "Governance", icon: "⚖", status: "building", text: "Proposals and votes that change the protocol in the open." },
  { id: "defsec", name: "Cyber defence", icon: "⛨", status: "building", text: "Shared threat intelligence recorded on-chain so defenders can verify it." },
  { id: "offsec", name: "Offensive security lab", icon: "⌖", status: "building", text: "Sandboxed, authorized-scope testing tools. Never for targets you do not own." },
  { id: "network", name: "Networking", icon: "⌁", status: "building", text: "Mesh and relay transport for places with poor connectivity." },
  { id: "ai", name: "AI network", icon: "✶", status: "planned", text: "Verified AI inference paid in MAYA, on open-weight models, run by miners' GPUs." },
  { id: "genetics", name: "Genetics", icon: "⧬", status: "planned", text: "Owner-controlled genomic data: you decide who reads it, and every access is recorded." },
  { id: "engineering", name: "Engineering & 3D", icon: "△", status: "planned", text: "Provenance for 3D models and designs; a market for GPU rendering and simulation." },
  { id: "simulation", name: "Simulation", icon: "∿", status: "planned", text: "Distributed scientific simulation jobs, with results anyone can check." },
  { id: "research", name: "Research & development", icon: "⚗", status: "planned", text: "Timestamped, citable research records and open data sets." },
  { id: "defense", name: "Civil defence", icon: "⛉", status: "planned", text: "Disaster-response coordination and resilient communications. Civil and defensive uses only." },
  { id: "space", name: "Space", icon: "✦", status: "research", text: "Satellite links and orbital timing research. Nothing in consensus depends on it yet." },
];
