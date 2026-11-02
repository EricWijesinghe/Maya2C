# Maya2C Improvement Proposals (MIPs)

A MIP is a full design for a protocol change that is too large for one ADR: architecture,
security model, economics, attack simulations and a roadmap. The decision itself is still
recorded as an ADR, which links to the MIP.

Rules:
- A MIP never changes code. Each consensus part it proposes ships behind an activation height
  of `u64::MAX` until its own spec rules, vectors, tests and reviews exist (ADR-002).
- Numbers in a MIP are placeholders until measured, and are marked MISSING (Standing Order 2).
- No superlative claims without a dated prior-art record (Standing Order 10).

| MIP | Title | Status | ADR |
|---|---|---|---|
| [0001](MIP-0001-sentinel-recovery-network.md) | Sentinel Recovery Network: threat levels, cell quarantine, forward-only governed recovery | Accepted (build after mainnet) | [077](../adr/ADR-077-sentinel-recovery-network.md) |
| [0002](MIP-0002-omega-durable-advantage.md) | Omega design: advantage numbers do not decide; three-speed evolution, bounded controllers, test ratchet, moat indicators | Accepted | [078](../adr/ADR-078-omega-durable-advantage.md) |
