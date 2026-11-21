# Runbook: incident log and status history

The status service (`apps/status`, `maya-status`) keeps an append-only record
of incidents at `/history`. Halts are drafted automatically from the chain's
own block timestamps. A person publishes them.

## What the log trusts

- `DIR/incidents.jsonl`: one JSON line per record. Each line stores the hash of
  the line before it, so an edit, a deletion, a reordering or an insertion
  breaks the chain at the first line it touched. `/api/incidents` reports
  `tampered` and shows no rows in that case.
- Drafts are written by the status service, unsigned. They say what the chain
  showed. They are never shown as published.
- Published records are signed with ML-DSA-65 by an operator key listed in
  `DIR/operators.txt` (one hex public key per line). A copy of the file cannot
  forge a published incident without that key.

Known limit: a truncated tail (newest lines removed) still verifies. Pin the
head hash that `maya-incident verify` prints somewhere off the machine.

## One-time setup

```sh
maya-incident keygen --out /secure/path/operator.key
# prints the public key; add it to DIR/operators.txt
```

Keep the seed file off the repository and out of logs. Restrict it to the
operator account (on Windows, the ACL; `chmod 600` on Unix). Anyone holding it
can publish incidents.

## Start the status service with a log

```sh
maya-status --node http://127.0.0.1:8545 --listen 127.0.0.1:3100 \
            --incidents /var/lib/maya-status/incidents \
            --metrics http://127.0.0.1:9100/metrics
```

`--operators FILE` overrides the default `DIR/operators.txt`. `--metrics` is the
node's `--metrics-addr` exporter; without it finality shows MISSING.

## Publish an incident

```sh
maya-incident publish --dir /var/lib/maya-status/incidents --key /secure/path/operator.key \
  --id halt-1793172992 --title "Chain halt, 212 s" \
  --body "Two validators restarted together; quorum returned at block 40123." \
  --report https://status.maya2c.dev/reports/2026-11-10-halt.md
```

For a new incident with no draft, add `--started UNIX_SECONDS`. The `--id` of
a drafted halt is `halt-<start time>`. `--ended` and `--started` default to the
logged times.

## Verify

```sh
maya-incident verify --dir /var/lib/maya-status/incidents
# verified N records, head <hash>
```

Record the head hash off the machine after every publication.

## When the log reports tampered

1. Do not delete the file. Copy it aside first.
2. `maya-incident verify` names the failing line and the reason.
3. Compare with the off-machine head hash. Restore from a copy whose head matches.
4. Find out who edited the file and why before publishing anything else.
