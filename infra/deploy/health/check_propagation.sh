#!/usr/bin/env bash
#
# Block propagation health: is this node's tip advancing, and is it level with
# a reference peer?
#
# NOT VERIFIED ON A LIVE SYSTEM.
#
# Exits 0 healthy, 1 degraded, 2 unreachable.
#
# --- what this measures, and what it does not -------------------------------
#
# It measures *height lag* against a reference node and *tip advance* over a
# sampling window. It does not measure true propagation latency — the time
# between one node accepting a block and another doing so — because that needs
# both nodes timestamped against a common clock, and `chain.rs` reads
# `header.timestamp` only for difficulty retargeting. There is no future-drift
# bound and no median-time-past, so a miner may write any u64 into it.
#
# A propagation figure derived from block timestamps would therefore be a
# number a miner can choose. Height lag cannot be forged the same way: it is
# two nodes' own views, read directly.

set -euo pipefail

RPC="${RPC:-http://127.0.0.1:8545}"
REFERENCE_RPC="${REFERENCE_RPC:-}"

# Seconds between the two height samples.
#
# 30s against a 15s target block time: long enough that a healthy chain
# advances at least once, short enough to be a useful alert interval. A window
# below the block time would report "stalled" every time it happened to land
# between blocks.
WINDOW="${WINDOW:-30}"

# Blocks behind a reference before this is called degraded.
#
# Two: one is ordinary — the reference may simply have seen the newest block
# first — and three or more means this node is not keeping up.
MAX_LAG="${MAX_LAG:-2}"

log() { printf '[propagation] %s\n' "$*"; }

height_of() {
    curl -fsS --max-time 5 -X POST -H 'content-type: application/json' \
        --data '{"jsonrpc":"2.0","id":1,"method":"get_supply","params":[]}' \
        "$1" >/dev/null 2>&1 || return 1

    curl -fsS --max-time 5 "${2:-http://127.0.0.1:9100/metrics}" 2>/dev/null \
        | awk '/^maya_chain_height /{print $2}' || return 1
}

start="$(height_of "$RPC" || true)"
if [ -z "$start" ]; then
    log "UNREACHABLE: no maya_chain_height"
    exit 2
fi
start="${start%.*}"
log "height now: $start"

log "sampling again in ${WINDOW}s"
sleep "$WINDOW"

end="$(height_of "$RPC" || true)"
if [ -z "$end" ]; then
    log "UNREACHABLE: node stopped answering during the window"
    exit 2
fi
end="${end%.*}"
advance=$((end - start))
log "height after ${WINDOW}s: $end (advanced $advance)"

status=0

if [ "$advance" -le 0 ]; then
    # Not automatically a fault: at a 15s target, a 30s window normally sees
    # one or two blocks, but variance is real and proof-of-work is Poisson.
    # Reported as degraded so a single sample is a warning rather than a page,
    # and a run of them is the signal.
    log "DEGRADED: the tip did not advance"
    log "one sample is weak evidence — proof-of-work interarrival is Poisson."
    log "alert on several consecutive failures, not on one"
    status=1
fi

if [ -n "$REFERENCE_RPC" ]; then
    ref="$(height_of "$REFERENCE_RPC" "${REFERENCE_METRICS:-}" || true)"
    if [ -z "$ref" ]; then
        log "warning: reference at $REFERENCE_RPC unreachable; lag not checked"
    else
        ref="${ref%.*}"
        lag=$((ref - end))
        log "reference height: $ref (lag $lag, maximum $MAX_LAG)"
        if [ "$lag" -gt "$MAX_LAG" ]; then
            log "DEGRADED: behind the reference"
            status=1
        elif [ "$lag" -lt "-$MAX_LAG" ]; then
            # Ahead of the reference by more than the tolerance. Worth saying:
            # it can mean the reference is stuck, or that these two nodes are
            # on different chains — which the peer check will not catch,
            # because a forked node still has peers.
            log "DEGRADED: AHEAD of the reference by $((-lag))"
            log "the reference may be stalled, or the two may be on different chains"
            status=1
        fi
    fi
fi

[ "$status" -eq 0 ] && log "OK"
exit "$status"
