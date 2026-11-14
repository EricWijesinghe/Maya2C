// Every app and field of the Maya2C ecosystem, with an honest stage.
// "built" means the code exists and ran on the public testnet; whether it is
// live right now is read from the node snapshot (status.js), never written
// here. Update a stage when it changes in features.toml. `icon` names a
// symbol in icons.svg.
"use strict";

const UNIVERSE = [
  { id: "wallet", name: "Maya2C Wallet", icon: "wallet", status: "built", text: "Post-quantum desktop wallet: send, receive, and keys kept in the OS keychain." },
  { id: "explorer", name: "Explorer", icon: "explorer", status: "built", text: "Every block, transaction and account of the testnet, at explorer.maya2c.dev." },
  { id: "node", name: "Validators", icon: "validators", status: "built", text: "The DAG-BFT committee that produces and finalizes blocks. Anyone can run a node." },
  { id: "mining", name: "Mining", icon: "mining", status: "building", text: "GPU and CPU miners paid from the block-reward reserve, on testnet MAYA that has no value." },
  { id: "chat", name: "Maya2C Chat", icon: "chat", status: "building", text: "Encrypted peer-to-peer chat. Messages wait with the sender until delivered, plus animated stickers." },
  { id: "dex", name: "Exchange", icon: "exchange", status: "building", text: "On-chain order book for swapping assets." },
  { id: "identity", name: "Identity", icon: "identity", status: "building", text: "One identity across every app, owned by you." },
  { id: "governance", name: "Governance", icon: "governance", status: "building", text: "Proposals and votes that change the protocol in the open." },
  { id: "defsec", name: "Cyber defence", icon: "shield", status: "building", text: "Shared threat intelligence recorded on-chain so defenders can verify it." },
  { id: "offsec", name: "Offensive security lab", icon: "target", status: "building", text: "Sandboxed, authorized-scope testing tools. Never for targets you do not own." },
  { id: "network", name: "Networking", icon: "mesh", status: "building", text: "Mesh and relay transport for places with poor connectivity." },
  { id: "ai", name: "AI network", icon: "spark", status: "planned", text: "Verified AI inference on open-weight models, run on miners' GPUs." },
  { id: "genetics", name: "Genetics", icon: "dna", status: "planned", text: "Owner-controlled genomic data: you decide who reads it, and every access is recorded." },
  { id: "engineering", name: "Engineering & 3D", icon: "cube", status: "planned", text: "Provenance for 3D models and designs; a market for GPU rendering and simulation." },
  { id: "simulation", name: "Simulation", icon: "wave", status: "planned", text: "Distributed scientific simulation jobs, with results anyone can check." },
  { id: "research", name: "Research & development", icon: "flask", status: "planned", text: "Timestamped, citable research records and open data sets." },
  { id: "defense", name: "Civil defence", icon: "buoy", status: "planned", text: "Disaster-response coordination and resilient communications. Civil and defensive uses only." },
  { id: "space", name: "Space", icon: "orbit", status: "research", text: "Satellite links and orbital timing research. Nothing in consensus depends on it yet." },
];
