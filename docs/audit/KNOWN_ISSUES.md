# Known issues, given to every auditor

What the team already knows, so audit time is not spent rediscovering it.
Each item links its evidence.

| # | Issue | Where | Status |
|---|---|---|---|
| 1 | ~~DAG-BFT engine not wired; the node runs PoW~~ **Resolved**: DAG-BFT in the node, live on maya-testnet-1 | ADR-027, `crates/node/tests/bft_node_tests.rs` | closed |
| 2 | ~~Staking and slashing not built~~ **Built**; voting is still one seat one vote — see 18 | ADR-028, `crates/node/tests/bft_staking_tests.rs` | closed |
| 3 | ~~Fee market inactive (`FeeConfig::DISABLED`)~~ **Resolved**: active from genesis on DAG-BFT chains | ADR-029 | closed |
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
| 15 | ~~Contracts cannot learn their caller~~ **Resolved 2026-09-27**: `caller` host function, live from genesis | ADR-026 (Accepted), `crates/reference-apps/tests/nft_game.rs`, `crates/node/tests/nft_game_on_node_tests.rs` | closed |
| 16 | Faucet grants once per IP per day; a venue behind one NAT gets one grant | `docs/ecosystem/HACKATHON_KIT.md` | open |
| 17 | ~~DAG-BFT quorum was 2f + 1 at every committee size: unsafe at n = 2, 3, 5, 6, …~~ **Fixed**: n − f | ADR-039, `crates/dag-bft/src/vertex.rs` | closed |
| 18 | ~~Committee capture by cheap absent seats~~ **Fixed for mainnet**: stake-weighted committees and checkpoints from a weighted genesis (ADR-040); maya-testnet-1 relies on `--min-register-bond`. Recovery from a genuine >1/3-stake outage still has no in-protocol path | ADR-039, ADR-040 | capture closed; recovery open |
| 19 | ~~Rejoin failures found by the self-run attacknet~~ **Fixed**: an f+1 crash left the restarted validators as permanent followers; no rejoin across an epoch boundary; the checkpoint threshold n − f deadlocked whenever more than f validators were followers (now f + 1); bootnodes never redialled; one HTTP 429 aborted a bootstrap | ADR-038 (2026-10-05 sections), `cargo xtask attacknet` | closed |
| 20 | Observer nodes follow by polling a peer's RPC for checkpoints and blocks, about 2 requests a second each, never over p2p. Twelve followers on one IP exhaust the public bootstrap endpoint's 10-per-second limit | ADR-038 | open |
