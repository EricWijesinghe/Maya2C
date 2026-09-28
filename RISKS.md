# Risk register

Reviewed every 10 sessions (Operating Protocol, CLAUDE.md). Likelihood and
impact are judgements, written as Low / Medium / High, not computed
numbers. Owner is who can act on it: **Eric** or **Claude** (the working
sessions). Seeded 2026-09-28; next review due at the 10th session after it.

| # | Risk | Likelihood | Impact | Current mitigation | Owner | Status |
|---|---|---|---|---|---|---|
| R1 | **Single person, key-person risk.** One owner and developer; if Eric stops, everything stops, and some knowledge exists only in chat sessions | High | High | Handover report (`reports/31-handover.md`), STATE.md every session, ADRs, runbooks. Not mitigated: no second maintainer, no key backup holder | Eric | Open |
| R2 | **Unverified features assumed to work.** A report says "met" and later work builds on it | High | High | `features.toml` gated by `cargo xtask coverage`; verification sweeps; downgrades recorded. Gap: sweeps are new (first one 2026-09-28) | Claude | Open |
| R3 | **Scope too large for the team.** 87 crates, 30 Master Prompts, a 160-prompt trajectory, one developer | High | High | ADR-016 launch scope freeze; milestone ladder with forbidden work (CLAUDE.md, Part C) | Eric | Open |
| R4 | **Funding and the cost of audits.** Mainnet needs external audits; estimates in the hundreds of thousands of dollars (estimates, not quotes) | High | High | Free-tier infrastructure plan; grant applications (NLnet draft for chat); nothing spent without `APPROVED:` | Eric | Open |
| R5 | **Legal and regulatory uncertainty.** Token, custody, exchange-like features (DEX, CBDC vault, bridges), and chat's abuse obligations, across jurisdictions | Medium | High | No token sale; legal questions listed in `reports/18-economics.md`; legal sign-off is a go/no-go NEEDS HUMAN gate. Not mitigated: no lawyer engaged | Eric | Open |
| R6 | **Unaudited third-party crates.** RocksDB bindings, libp2p, arkworks, PQ crates; `cargo vet` not set up | Medium | High | `cargo deny` (advisories, licences, sources) in CI; pinned versions; geiger survey reported | Claude | Open |
| R7 | **Post-quantum library immaturity.** Young implementations; HQC decapsulation measured leaking (\|t\| 47.5) so HQC stays draft; constant-time claims rest on our own dudect runs | Medium | High | Hybrid signatures (ML-DSA + SLH-DSA) and X-Wing combiner, secure if either half is; suite registry allows rotation (invariants 29, 30); dudect results in `reports/` | Claude | Open |
| R8 | **No independent review of consensus-critical code.** Every review so far is by the same model family that wrote it | High | High | Invariant tests, Kani, conformance vectors, fuzzing in CI. Not mitigated: CLAUDE.md asks for two human reviewers on consensus changes; there are none | Eric | Open |
| R9 | **Launch timing pressure.** Pressure to announce or launch before M6/M7 criteria are met | Medium | High | Go/no-go computed from evidence (`cargo xtask go-no-go`, today NO-GO); claims-check blocks superlatives | Eric | Open |
| R10 | **A competitor ships the same differentiator first.** Already true for PQ chat encryption (Signal, iMessage, SimpleX — `docs/prior-art/p2p-chat.md`); PQ signatures from genesis may follow | High | Medium | Prior-art files with dated searches before any claim; differentiation stated per app in ECOSYSTEM.md | Eric | Open |
| R11 | **Parallel sessions on one checkout.** Two sessions sharing `D:\Maya2C` and one `target/` collided on 2026-09-28 (edits and a commit on another session's branch were narrowly avoided) | Medium | Medium | Separate git worktree per session (`git worktree add`); message the other session before a long build | Claude | Open |
| R12 | **Disk and memory exhaustion on the build machine.** Volume filled to zero twice; commit-charge exhaustion (os error 1455) | Medium | Medium | `cargo xtask disk`; `jobs = 4`; separate target dirs cost disk, checked before each sweep | Claude | Open |
