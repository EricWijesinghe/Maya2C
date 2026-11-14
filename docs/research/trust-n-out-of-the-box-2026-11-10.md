# Prior-art notes: Out of the box (CATALOG section N)

Research behind [CATALOG section N](../ecosystem/CATALOG.md), searched
2026-11-10 from search results only. Nothing here is a novelty claim: a row
needs its own dated `docs/prior-art/` record before any "others don't"
wording (Standing Order 10). Links are recorded as found, not re-fetched.

## Prior-art notes (searched 2026-11-10)
- flight-recorder: https://github.com/anza-xyz/agave/wiki/Debugging-Consensus-Failures, https://solana.com/news/09-30-22-solana-mainnet-beta-outage-report
- bench-rerun: https://crypto.news/blockchain-performance-overstated-by-20x-taraxa-report-finds (author has a conflict), https://hackernoon.com/sui-hits-406-million-tps-live-in-singapore-with-certik-as-the-referee, https://cryptorank.io/news/feed/72dac-venom-foundation-proposes-standardized-blockchain-tps-benchmark
- explain-block: https://goldrush.dev/blog/introducing-goldrush-decoder-decode-visualize-understand/
- ai-receipts: https://datatracker.ietf.org/doc/html/draft-tsyrulnikov-rats-attested-inference-receipt-00, https://ftp.kaist.ac.kr/ietf/draft-rotzin-spice-afir-profile-00.html, https://ethereum-magicians.org/t/erc-8299-wyriwe-what-you-read-is-what-you-execute/28655
- exploit-bounty-ledger: https://www.usenix.org/conference/usenixsecurity18/presentation/breindenbach
- phone-watchers: https://forum.celestia.org/t/celestia-community-roadmap/1806
- censorship-monitor: https://arxiv.org/pdf/2305.18545.pdf, https://research.lido.fi/t/rfp-ethereum-censorability-monitor/3330/16
- trust-facts: https://l2beat.com/stages

## Rejected ideas (13), with the reason

- ZK proof the founder holds no hidden keys: non-possession cannot be proven; the ceremony sheet plus disclosed founder mining is the honest form.
- Verifiable carbon/energy accounting: energy is modelled (`xtask/src/bench_report.rs:7-10`, joules = seconds x 45 W), so a number would break Standing Orders 1-2.
- DR drills as a separate row: merged into flight-recorder.
- claims-check as a public service: no trust gain for this chain.
- Multi-watcher site-hash attestation: `xtask/src/site_integrity.rs` covers it.
- Key-ceremony log: covered by J ceremony-transcript.
- Upgrade diff explainer: covered by M upgrade-explainer.
- Validator SLA scorecards: D validator-console.
- Public guard-trip alerts: sentinel + ntfy.
- On-chain warrant canary: legal; kept in J only as foundation-canary with legal review.
- Founder dead-man switch over council powers: the council is limited by invariant 34 and `crates/node/src/state/council.rs` (pause and sunset only).
- Public proof-ratchet submissions: merged into exploit-bounty-ledger.
- Anchoring evidence and sweep hashes: the J release-log-anchor/notary row applied to `reports/`.
