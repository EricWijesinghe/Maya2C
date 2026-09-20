#!/usr/bin/env bash
#
# Peer connection health.
#
# NOT VERIFIED ON A LIVE SYSTEM — no Linux and no shellcheck on the authoring
# host. The arithmetic is unit-testable and is tested in
# `infra/deploy/health/tests.sh`; the live RPC calls are not.
#
# Exits 0 healthy, 1 degraded, 2 unreachable. Distinct codes because they call
# for different actions: degraded is a network problem, unreachable is a
# process problem, and an alert that cannot tell them apart pages the wrong
# person.

set -euo pipefail

RPC="${RPC:-http://127.0.0.1:8545}"
METRICS="${METRICS:-http://127.0.0.1:9100/metrics}"

# Below this, a node is not meaningfully connected.
#
# Two, not one: a single peer is a single point of failure and, more to the
# point, a node with exactly one peer cannot tell a fork from the truth. It is
# the number `docs/mainnet-readiness.md` already uses for the same check.
MIN_PEERS="${MIN_PEERS:-2}"

log() { printf '[peers] %s\n' "$*"; }

# `maya_peers_connected` comes from the metrics exporter, which is bound to
# loopback. Falling back to nothing rather than to a guess: a health check that
# invents a number when it cannot measure one is worse than one that fails.
peers="$(curl -fsS --max-time 5 "$METRICS" 2>/dev/null \
    | awk '/^maya_peers_connected /{print $2}' || true)"

if [ -z "$peers" ]; then
    log "UNREACHABLE: no maya_peers_connected at $METRICS"
    log "the node may be down, or --metrics-addr may not be set"
    exit 2
fi

# Metrics are floats; compare as integers.
peers_int="${peers%.*}"

log "connected peers: $peers_int (minimum $MIN_PEERS)"

if [ "$peers_int" -lt "$MIN_PEERS" ]; then
    log "DEGRADED: below the minimum"
    log "check: bootnodes reachable, UFW allows the P2P port inbound,"
    log "       and the genesis state root matches the rest of the fleet —"
    log "       a node on the wrong genesis connects to nobody and looks"
    log "       exactly like a firewall problem"
    exit 1
fi

log "OK"
exit 0
