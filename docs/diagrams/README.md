# Diagrams

Five pictures of Maya2C, each in two forms: a hand-drawn SVG in the Forged
Light palette (teal `#2cc3cf`, bronze `#d9b97a`, steel `#b9c6ca` on abyss
`#050b0f`) for the README and the site, and a Mermaid source below that GitHub
renders and anyone can edit in a pull request.

Every box names a real crate, file or function. When the code moves, change
the Mermaid source and the SVG in the same pull request. The status chips in
the ecosystem map are `features.toml` values; change them when the ledger
changes, never ahead of it.

| Diagram | SVG | Checked against |
|---|---|---|
| System architecture | [architecture.svg](architecture.svg) | `crates/node/src/{network,rpc,state}`, `crates/dag-bft`, `crates/vm`, `crates/api-gateway` |
| Transaction lifecycle | [tx-lifecycle.svg](tx-lifecycle.svg) | `HybridVerifyingKey::verify` (`crates/node/src/crypto/hybrid.rs`), `Mempool::insert`, `StateDB::apply_block_journaled` |
| Trust boundaries | [trust-boundaries.svg](trust-boundaries.svg) | `crates/api-gateway/src/allowlist.rs`, `crates/node/src/rpc/limit.rs`, `crates/dos-guard`, `docs/pq-transport.md`, `bins/maya2c-signer` |
| Ecosystem map | [ecosystem.svg](ecosystem.svg) | `features.toml` subsystem entries for each app |
| Milestone ladder | [roadmap.svg](roadmap.svg) | [docs/MISSION.md](../MISSION.md), [ADR-042](../adr/ADR-042-solo-launch-path.md) |

## System architecture

![System architecture](architecture.svg)

```mermaid
flowchart TB
  classDef app fill:#0c171d,stroke:#2cc3cf,color:#eef3f3
  classDef core fill:#12232b,stroke:#8ff0f5,color:#eef3f3
  classDef pq fill:#1d1709,stroke:#d9b97a,color:#f4ead6
  subgraph Apps
    W["Maya2C Wallet<br/>apps/wallet-gui"]:::app
    CH["Maya2C Chat<br/>apps/chat-gui"]:::app
    H["Maya2C Hub<br/>apps/hub"]:::app
    E["Explorer<br/>apps/explorer"]:::app
    SDK["SDKs<br/>sdks/"]:::app
    MCP["MCP server<br/>bins/maya2c-mcp"]:::app
  end
  subgraph Edge
    GW["API gateway<br/>default-deny allowlist"]:::app
    RPC["Node JSON-RPC<br/>per-IP rate limit"]:::app
  end
  subgraph Node["Node: bins/maya2c-node + crates/node"]
    MP["Mempool<br/>network/mempool.rs"]:::core --> BFT["DAG-BFT<br/>crates/dag-bft"]:::core
    BFT --> VM["WASM VM<br/>crates/vm"]:::core
    VM --> ST["State + state root<br/>crates/node/src/state"]:::core
  end
  P2P["libp2p TCP + WebSocket<br/>ML-KEM-1024, 768 fallback, over Noise"]:::pq
  CR["ML-DSA-65 + SLH-DSA-SHA2-128s<br/>crates/crypto-pq · ledger-math"]:::pq
  Apps --> Edge --> Node
  Node <--> P2P
  Node --> CR
```

## Transaction lifecycle

![Transaction lifecycle](tx-lifecycle.svg)

```mermaid
sequenceDiagram
  participant W as Maya2C Wallet
  participant G as API gateway
  participant M as Mempool
  participant P as Peers (libp2p)
  participant B as DAG-BFT
  participant S as StateDB
  W->>W: sign signing_bytes(chain tag) with ML-DSA-65 and SLH-DSA
  W->>G: submit (allowlisted method)
  G->>M: node JSON-RPC
  M->>M: Mempool::insert: verify both, nonce, fee, balance, breaker
  M->>P: gossip
  P->>B: vertices, Bullshark commit
  B->>S: apply_block_journaled
  S->>S: verify both again, run VM, recompute state root
  alt root differs (invariant 24)
    S-->>B: refused, nothing written
  else root matches
    S->>S: block and state in one WriteBatch (invariant 26)
  end
```

The hybrid check inside `HybridVerifyingKey::verify`:

```mermaid
flowchart LR
  classDef pq fill:#1d1709,stroke:#d9b97a,color:#f4ead6
  classDef ok fill:#0b1f17,stroke:#4fe3a5,color:#eef3f3
  classDef bad fill:#1f0d0b,stroke:#ff7a6b,color:#eef3f3
  SB["signing_bytes<br/>both keys + chain tag"] --> L["ML-DSA-65 verify<br/>runs first"]:::pq
  L -->|ok| HB["SLH-DSA-SHA2-128s verify"]:::pq
  L -->|fail| R1["SignatureVerification"]:::bad
  HB -->|ok| A["accepted"]:::ok
  HB -->|fail| R2["HashSignatureVerification"]:::bad
```

## Trust boundaries

![Trust boundaries](trust-boundaries.svg)

```mermaid
flowchart LR
  classDef bad fill:#1f0d0b,stroke:#ff7a6b,color:#eef3f3
  classDef zone fill:#0c171d,stroke:#2cc3cf,color:#eef3f3
  classDef key fill:#1d1709,stroke:#d9b97a,color:#f4ead6
  U["Untrusted<br/>clients, peers, other geneses"]:::bad
  subgraph Edge["Public edge"]
    A1["gateway allowlist"]:::zone
    A2["per-IP rate limit"]:::zone
    A3["dos-guard: cheap check first"]:::zone
  end
  subgraph Peer["Peer edge"]
    B1["Noise prologue bound to genesis"]:::zone
    B2["ML-KEM layer"]:::zone
    B3["gossip scoring"]:::zone
  end
  subgraph Cons["Consensus"]
    C1["both signatures"]:::zone
    C2["state root (inv 24)"]:::zone
    C3["block byte cap (inv 33)"]:::zone
    C4["invariant guard (inv 28)"]:::zone
  end
  subgraph Keys
    K1["maya2c-signer, loopback"]:::key
    K2["sealed key file"]:::key
    K3["ZeroizeOnDrop secrets"]:::key
  end
  U --> Edge --> Cons
  U --> Peer --> Cons
  Cons --> Keys
```

External audit: pending. `CIRCUIT_IS_AUDITED = false` in
`crates/zk-stark/src/pool/mod.rs`.

## Ecosystem map

![Ecosystem map](ecosystem.svg)

```mermaid
flowchart TB
  classDef chain fill:#12232b,stroke:#d9b97a,color:#eef3f3
  classDef v fill:#0b1f17,stroke:#4fe3a5,color:#eef3f3
  classDef w fill:#0b1a24,stroke:#8fb4ff,color:#eef3f3
  classDef p fill:#1a1a1a,stroke:#8a9ca3,color:#eef3f3
  C(("Maya2C chain")):::chain
  C --- W["Maya2C Wallet · verified"]:::v
  C --- CH["Maya2C Chat · working, research"]:::w
  C --- H["Maya2C Hub · planned"]:::p
  C --- E["Explorer · verified"]:::v
  C --- L["Ledger app · working"]:::w
  C --- S["SDKs · verified"]:::v
  C --- M["MCP server · working"]:::w
  M --- AG["AI agents: read and draft, never sign"]:::w
  C --- SE["Sentinel · working"]:::w
```

## Milestone ladder

![Milestone ladder](roadmap.svg)

```mermaid
flowchart LR
  M0[M0 Truth] --> M1[M1 Core solid] --> M2[M2 Fast and proven] --> M3[M3 Usable] --> M4[M4 Connected]
  M4 --> M5[M5 Operable] --> M6[M6 Public testnet] --> M7[M7 Audited] --> M8[M8 Mainnet] --> M9[M9 Ecosystem]
```

[ADR-042](../adr/ADR-042-solo-launch-path.md) allows mainnet to launch
labelled "external audit pending" before the M7 report lands. Where the
project stands now: [STATE.md](../../STATE.md).
