# Audit findings tracker

Every finding gets a row. It is closed only when it has a fix commit, a
regression test, and a re-review by the firm that raised it. `cargo xtask
go-no-go` reads this table. Any row with severity `critical` or `high` that
is not `closed` is a NO-GO, and so is an empty table: no findings means no
audit, not a clean one.

| ID | Scope | Severity | Title | Issue | Fix commit | Regression test | Re-review | Status |
|---|---|---|---|---|---|---|---|---|

(No external audit has reported. Rows start at `F-001`.)
