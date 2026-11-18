# Runbook: Live chain sentinel (`maya2c-sentinel`)

**What it is.** A small read-only watchdog (`apps/sentinel`, ADR-077). It polls
each node's RPC every 5 s, runs the detectors below, and sends one phone push
per incident, plus a `RESOLVED` push when the condition clears. It never signs,
never submits transactions and never changes chain state. Every alert is a
message to a person, and the response is one of the steps in this file or a
linked runbook.

**Latency.** A halt is pushed about 35 s after the last block (30 s window plus
one 5 s poll). An unreachable node is pushed after 45 s.

## Run it

```text
# Phone topic, from your ntfy app. Set it in the environment, never on the command line.
export MAYA2C_SENTINEL_NTFY_TOPIC=<your-private-topic>

maya2c-sentinel --node seed=http://127.0.0.1:8545 \
                --node v2=http://127.0.0.1:8546 \
                --node v3=http://127.0.0.1:8547 \
                --log /var/log/maya2c-sentinel.log
```

Check the wiring without sending anything:

```text
maya2c-sentinel --node seed=http://127.0.0.1:8545 --dry-run
```

Dry-run prints each push as `[dry-run] P<n> <title> :: <body>` and still
writes the log. It needs no topic.

Flags (defaults in brackets): `--poll-secs` [5], `--halt-secs` [30],
`--unreachable-secs` [45], `--min-peers` [2], `--strand-blocks` [20],
`--strand-secs` [120], `--finality-lag-blocks` [30], `--liveness-secs` [60],
`--heartbeat PATH` [off], `--log` [`maya2c-sentinel.log`], `--dry-run`.

## Dead-man heartbeat

A sentinel that has crashed sends nothing, and silence looks like a healthy
chain. Run it with `--heartbeat PATH`. It writes the Unix time to `PATH` after
every poll. Then schedule a check every minute, **on a different machine if
you can** (a down machine cannot page about itself):

```text
# cron: * * * * *   (Windows: Task Scheduler, every 1 minute)
MAYA2C_SENTINEL_NTFY_TOPIC=<your-private-topic> \
  maya2c-sentinel --check-heartbeat /var/lib/maya2c/sentinel.beat --max-age-secs 120
```

The check pushes **Maya2C sentinel stopped** (Critical) when the file is
missing, unreadable or older than `--max-age-secs` [120], and exits 2. It
writes `PATH.alerted` with the stamp it paged on, so it pages once per outage.
`--dry-run` prints instead of pushing.

- **Check:** is the sentinel process running (find it by PID, never by
  image name)? Read the end of its `--log`.
- **Act:** restart it. Until it runs again, nothing is watching the chain, so
  check `get_chain_info` by hand.

The process refuses to start live without a valid topic. A topic is 1 to 64
letters, digits, `-` or `_`. Anything else is rejected, because a bad topic
means alerts that go nowhere.

## Subscribe on the phone

1. Install ntfy from the Play Store or the App Store.
2. Tap **+**, choose **Subscribe to topic**, and enter the same topic string
   as `MAYA2C_SENTINEL_NTFY_TOPIC`. Use a long random topic. Anyone who knows
   it can read the alerts.
3. Turn on notifications for the app. Critical alerts (priority 5) are the
   urgent kind; allow the app to override Do Not Disturb if you want them at night.
4. Send a test: `curl -d "test" -H "Title: sentinel test" https://ntfy.sh/<topic>`.
   Run it from a machine that knows the topic, never paste the topic into a
   chat or a repository.

## Alerts

Priority maps from severity: **5** Critical (urgent, the loudest ntfy
notification), **4** Warning (high), **3** RESOLVED.

Every alert has the same first step: check whether the problem is this
observer or the network. Ask a second operator, or open a second explorer,
before acting.

### Chain halted (`halt`, Critical)

No node has produced a new tip for 30 s (or the `--halt-secs` value).

- **Check:** `get_chain_info` on two nodes. Same height and stuck means the network.
  One node stuck means that node, see *Node unreachable* and `fell-behind.md`.
- **Act:** a network-wide halt follows `chain-halt.md` and, for a committee that
  cannot form quorum, its DAG-BFT section. Do not restart validators one by one
  before the committee is known.

### Fork (`fork:<height>`, Critical)

Two nodes report different block ids at the same height. The push names both ids.

- **Check:** which nodes are on each side, and whether a finalized block is in
  dispute. Use `state-root-mismatch.md` and `deep-reorg.md`.
- **Act:** stop signing on the minority side if it is a validator and let the
  committee decide. Do not reorganize the chain by hand. Chain state never
  rewinds (ADR-077).

### Supply does not add up (`supply-mismatch:<node>`, Critical)

`circulating + burned` differs from `total` on one node. This is a
conservation failure.

- **Check:** the node's `get_supply` output, and whether it was just restarted
  mid-upgrade.
- **Act:** `invariant-guard-tripped.md`. Keep the node up for evidence. Escalate
  to Eric before any action that changes state.

### Supply split (`supply-split:<height>`, Critical)

Two nodes at the same height report different totals. The push names the height.

- **Check:** as for a fork: which nodes, and whether their tips agree. A split
  with the same tip id is a supply bug, and needs the state-root runbook.
- **Act:** same as a fork.

### Supply fell (`supply-decrease:<node>`, Warning)

Total supply dropped at a height at or above the last one seen for that node.

- **Check:** a slashing event is a legitimate cause. Compare with the validator
  set and any recent evidence.
- **Act:** if no slash explains it, treat as a supply mismatch.

### Peer count collapsed (`peers:<node>`, Warning)

The node reports fewer peers than `--min-peers` (default 2).

- **Check:** `peer-starvation.md`, then `eclipse-suspected.md` if the peers
  that remain look the same.
- **Act:** a single isolated node is a fix to its bootnodes. Several isolated
  nodes at once point at the network, so treat it as a halt.

### Node unreachable (`rpc:<node>`, Warning)

No answer to `get_chain_info` for 45 s.

- **Check:** the process is running, the port is open, and the tunnel or
  forward is up. Watch for an RPC rate limit: a flood of 429s is the limiter,
  see `rpc-overload.md`, not an outage.
- **Act:** if the node is a validator, `signer-unreachable.md` and
  `validator-start-guards.md` before a restart. Never kill by image name: stop
  the process by its PID.

### Transactions stranded (`stranded`, Critical)

Transactions are waiting in the mempool while the chain finalizes blocks that
carry none of them: 20 empty blocks in a row (`--strand-blocks`), or 120 s
of waiting with no block carrying one (`--strand-secs`). This is the pattern
the benchmark found, where consensus messages were dropped and transactions
stayed stuck.

- **Check:** `get_bft_status` on each node. Compare `round` with
  `committed_round`, and look for a gap that does not close. Check
  `mempool-full.md` for the mempool side.
- **Act:** `chain-halt.md`, then the stuck-upgrade runbook if an upgrade
  height is near. Do not drop mempool entries by hand.

**Coverage.** The detector reads `get_mempool_info` (`{size, bytes}`, node-local,
not forwarded by the public gateway). It runs for every node that answers. For a
node that does not, the log carries `NOTE stranding idle for <node>` at start,
and that node cannot raise this alert. Before ADR-101 the sentinel read a
`pending` field the node never served, so stranding was idle on every node.

### Sentinel blind (`blind`, Critical)

None of the watched nodes has answered for 45 s (`--unreachable-secs`). Every
other alert is silent now, and that silence means nothing.

- **Check:** the sentinel machine's own network, tunnels and forwards first,
  then the nodes from another machine.
- **Act:** if the nodes answer from elsewhere, fix the sentinel's path to
  them. If they do not, treat it as a halt (`chain-halt.md`).

### Finality lagging (`finality-lag`, Warning)

The highest tip is 30 blocks or more (`--finality-lag-blocks`) past the newest
checkpoint (`get_checkpoint`) any watched node holds. Blocks are still being
made, but `2f + 1` attestations are not forming, so a node that falls behind
has nothing to catch up to (ADR-038).

- **Check:** `get_checkpoint` and `get_bft_status` on every validator: is
  each one in the committee, and is any of them a `follower`? Look for
  attestation gossip errors in the validator logs.
- **Act:** `fell-behind.md` for a validator that is not attesting. Do not
  restart several validators at once (the 2026-10-18 halt).

### Validator not proposing (`liveness:<node>`, Warning)

A watched committee member's `round` has not changed for 60 s
(`--liveness-secs`) while the network's committed round has moved. The push
says if the node is following attested blocks instead of building.

- **Check:** the node's `get_bft_status` (`validator`, `round`, `follower`),
  its signer (`signer-unreachable.md`) and its peers.
- **Act:** a follower that does not return to building: `fell-behind.md`. Only
  nodes the sentinel polls are covered; a committee member it does not watch
  cannot raise this alert.

### Resolved (`RESOLVED: <title>`, priority 3)

The condition has been gone for 30 s. Record the time in the incident log
and check that the chain is producing blocks.

## Not detected

- **Equivocation and slashing evidence.** Not exposed over RPC yet, so the
  sentinel cannot see it. Watch the validator logs and `get_validators` by hand.
- **Anything inside one node that its RPC does not expose.**
- **Supply audit (#454).** The `maya2c-audit supply` recompute is not wired
  in yet; #454 was not merged when ADR-101 was written.

## Logs and files

The log (`--log`, one line per event) records each push, each suppressed
push and each failed send. Suppression is rate-limited to 6 pushes per 10
minutes, so a flapping incident cannot flood the phone. Resolves are never
suppressed. A `SEND_FAILED` line means the push did not reach ntfy: check
outbound HTTPS and the topic, then read the log for the alert.

## Rehearsal

`cargo test -p maya-sentinel` runs the detectors against scripted nodes,
including the stranding replay (`apps/sentinel/tests/stranding_replay.rs`)
and the netfault scenarios (`apps/sentinel/tests/fault_scenarios.rs`: 2|2 and
3|1 partition with heal, latency with jitter, bursty loss, lost attestations).
Those replay modelled RPC answers; they are not a run of the relay.
