# Accepted performance regressions

`scripts/bench_gate.py` fails a run when a tracked metric is more than 5 %
worse than the last value recorded for the same runner in `history.csv`. A
change that is slower on purpose — a correctness fix, a security check added
to a hot path — lists the metric here, in backticks, with the reason and the
PR. The gate then reports that metric but does not fail on it.

Remove the line once the new value is the recorded baseline.

| Metric | Reason | PR |
|---|---|---|
