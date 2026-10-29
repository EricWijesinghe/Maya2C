# 2026-10-29 — mainnet build merged

Merged to master: #221 (ADR-068 forever mining), #222 (ML-DSA-87 validator keys, CRY-4),
#230 (24 decimals), #236 (pre-audit S-004 chain-tagged votes), #226, #233; peer: #235, #237.

Evidence (local, real output):
- feat/validator-mldsa87 after master merge: `cargo check --workspace --all-targets` — no errors.
- feat/decimals-24: `cargo nextest run -p maya-api-gateway` — `62 tests run: 62 passed`
  (after updating the supply-text test for 10^24); CI clippy command — 0 errors;
  `cargo nextest run -p maya-spec-ref` — `8 tests run: 8 passed`.
- fix/preaudit-chaintag: consensus tests `99 tests run: 99 passed`.
- Attacknet (peer, self-run): PASS, `reports/attacknet/2026-10-28-1.md`.

NOT VERIFIED here: full workspace nextest after the final merges — master CI on 299acd9d was
still running when written.
