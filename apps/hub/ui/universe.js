// Every app and field of the Maya2C ecosystem, with an honest stage.
// "built" means the code exists and ran on the public testnet before its pause
// on 2026-10-28; whether it is live right now is read from the node snapshot
// (status.js), never written here. Update a stage when it changes in features.toml. `icon` names a
// symbol in icons.svg. Anything not built says so in its own sentence.
"use strict";

const UNIVERSE = [
  { id: "wallet", name: "Maya2C Wallet", icon: "wallet", status: "built", text: "Post-quantum desktop wallet: send, receive, and keys kept in the OS keychain." },
  { id: "explorer", name: "Explorer", icon: "explorer", status: "built", text: "Kept every block, transaction and account of the testnet until its pause on 2026-10-28." },
  { id: "node", name: "Validators", icon: "validators", status: "built", text: "The DAG-BFT committee that produced and finalized the testnet's blocks. The node software is open source." },
  { id: "mining", name: "Mining", icon: "mining", status: "building", text: "Being built: the miner here runs on the testnet only. Mainnet mining waits for its genesis." },
  { id: "chat", name: "Maya2C Chat", icon: "chat", status: "building", text: "Being built: encrypted peer-to-peer chat where messages wait with the sender until delivered." },
  { id: "dex", name: "Exchange", icon: "exchange", status: "building", text: "Being built: an on-chain order book for swapping assets." },
  { id: "identity", name: "Identity", icon: "identity", status: "building", text: "Being built: one identity across every app, held by its owner." },
  { id: "governance", name: "Governance", icon: "governance", status: "building", text: "Being built: proposals and votes for changing the protocol in the open." },
  { id: "defsec", name: "Cyber defence", icon: "shield", status: "building", text: "Being built: shared threat intelligence recorded on chain, so defenders can check it." },
  { id: "offsec", name: "Offensive security lab", icon: "target", status: "building", text: "Being built: sandboxed testing tools for authorized scopes only, never for targets you do not own." },
  { id: "network", name: "Networking", icon: "mesh", status: "building", text: "Being built: mesh and relay transport for places with poor connectivity." },
  { id: "ai", name: "AI network", icon: "spark", status: "planned", text: "Planned: verified AI inference on open-weight models, to run on miners' GPUs." },
  { id: "genetics", name: "Genetics", icon: "dna", status: "planned", text: "Planned: genomic data its owner controls, with a record of each read the owner allows." },
  { id: "engineering", name: "Engineering & 3D", icon: "cube", status: "planned", text: "Planned: provenance for 3D models and designs, and GPU rendering jobs." },
  { id: "simulation", name: "Simulation", icon: "wave", status: "planned", text: "Planned: distributed scientific simulation jobs with checkable results." },
  { id: "research", name: "Research & development", icon: "flask", status: "planned", text: "Planned: timestamped, citable research records and open data sets." },
  { id: "defense", name: "Civil defence", icon: "buoy", status: "planned", text: "Planned: disaster-response coordination and resilient communications, for civil and defensive uses only." },
  { id: "space", name: "Space", icon: "orbit", status: "research", text: "Research: satellite links and orbital timing. Nothing in consensus depends on it." },
];
