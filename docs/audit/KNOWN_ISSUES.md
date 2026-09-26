# Known issues, given to every auditor

What the team already knows, so audit time is not spent rediscovering it.
Each item links its evidence.

| # | Issue | Where | Status |
|---|---|---|---|
| 1 | DAG-BFT engine not wired; the node runs PoW | ADR-015, `bins/maya2c-node` refuses a production start | blocks launch |
| 2 | Staking and slashing not built | `spec/06-staking.md` | blocks launch |
| 3 | Fee market inactive (`FeeConfig::DISABLED`) | ADR-016 | blocks launch |
| 4 | `apply_block` re-verifies every signature (93 % of apply time) | `reports/12-performance.md` §1 | designed, not built |
| 5 | Shielded-pool circuit unaudited | `CIRCUIT_IS_AUDITED = false` | deferred |
| 6 | HQC decapsulation leaks timing | ADR-009 | RESEARCH feature only |
| 7 | Threshold-lattice keygen is our own construction | ADR-014 | RESEARCH |
| 8 | Node–signer channel is a self-written AKE, not reviewed | ADR-022 | audit scope (a) |
| 9 | Headers carry no protocol version | ADR-020 | accepted for v1 |
| 10 | RPC, finality, sync and missed-round metrics not emitted | `reports/19-operations.md` | open |
| 11 | Mempool has no per-sender cap or fee-bump rule in the node | `reports/16-validator-security.md` | open (policy exists in `dos-guard`) |
| 12 | Kani not run this session; Aeneas translation not done | `reports/08-security.md` | open |
| 13 | `docs/sealed-mempool.md` referenced by `genesis.rs` does not exist | `reports/18-economics.md` | open |
| 14 | SHA3 re-commitment timing looks unoptimized under the `ci` profile | `reports/15-spec.md` §5 | not investigated |
| 15 | Contracts cannot learn their caller: no `caller` host function, so ownership cannot be checked | ADR-026 (Proposed), `crates/reference-apps/tests/nft_game.rs` | blocks every ownership contract |
| 16 | Faucet grants once per IP per day; a venue behind one NAT gets one grant | `docs/ecosystem/HACKATHON_KIT.md` | open |
