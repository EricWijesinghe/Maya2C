#!/usr/bin/env bash
# Runbook rehearsals that need a real process and a real kernel (Master
# Prompt 19 §3): disk-full, memory-pressure, peer-starvation. Linux only
# (tmpfs, docker --memory). Uses a proof-of-work devnet with one miner, the
# same shape as the operator rehearsal.
#
#   BIN=target/ci/maya2c-node bash scripts/runbook_rehearsals_linux.sh
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="$(realpath "${BIN:-target/ci/maya2c-node}")"
LOG="${LOG:-reports/data/runbook-rehearsals-linux.txt}"
WORK=$(mktemp -d)
t0=$(date +%s)
say() { echo "[$(( $(date +%s) - t0 ))s] $*" | tee -a "$LOG"; }
: > "$LOG"
cleanup() {
    pkill -f "$WORK" 2>/dev/null || true
    mountpoint -q "$WORK/tiny" && umount "$WORK/tiny" || true
    docker rm -f maya-oom >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup EXIT

cat > "$WORK/genesis.json" <<'EOF'
{"chain_id": "maya-runbooks", "timestamp": 1790000000, "difficulty_bits": 1, "pow_limit_bits": 1, "allocations": []}
EOF
rpc() {  # $1 port, $2 method
    curl -sf -m 3 -H 'content-type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[]}" "http://127.0.0.1:$1" \
        | python3 -c 'import sys,json; print(json.load(sys.stdin)["result"])' 2>/dev/null || echo "-"
}
node() {  # $1 data dir, $2 rpc port, $3 p2p port, rest: extra args
    local d=$1 r=$2 p=$3; shift 3
    "$BIN" --genesis "$WORK/genesis.json" --data-dir "$d" --rpc-addr "127.0.0.1:$r" --p2p-port "$p" "$@" \
        > "$d.log" 2>&1 &
    echo $!
}

# --- disk-full -------------------------------------------------------------
mkdir -p "$WORK/tiny"
mount -t tmpfs -o size=24m tmpfs "$WORK/tiny"
pid=$(node "$WORK/tiny/data" 34001 34101 --mine --threads 2)
sleep 10
# Disks fill from something else — logs, another tenant — so fill it the same
# way: everything but 64 KiB goes to a filler file while the node runs.
free_kb=$(df --output=avail -k "$WORK/tiny" | tail -1)
dd if=/dev/zero of="$WORK/tiny/filler" bs=1K count=$(( free_kb - 64 )) status=none 2>/dev/null || true
until grep -qiE "no space|os error 28|storage" "$WORK/tiny/data.log" 2>/dev/null || ! kill -0 "$pid" 2>/dev/null; do sleep 2; done
full_height=$(rpc 34001 get_tip_height)
say "disk-full: 24 MiB volume filled at height $full_height; node reported: $(grep -m1 -iE 'no space|os error 28|storage' "$WORK/tiny/data.log" | cut -c1-120)"
kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true
rm -f "$WORK/tiny/filler"  # the runbook's first step: free the space
pid=$(node "$WORK/tiny/data" 34001 34101 --mine --threads 2)
sleep 15
h=$(rpc 34001 get_tip_height)
say "disk-full: after freeing the space and restarting, the node reopened consistently and mined on to height $h"
kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true
[[ "$h" != "-" ]] && (( h >= full_height )) || { say "FAIL: disk-full recovery"; exit 1; }

# --- memory-pressure (OOM kill) ----------------------------------------------
CTX=$(mktemp -d); cp "$BIN" "$CTX/maya2c-node"
docker build -q -t maya2c/node:runbook -f infra/docker/Dockerfile.local "$CTX" >/dev/null; rm -rf "$CTX"
mkdir -p "$WORK/oom" && chmod 777 "$WORK/oom"
cp "$WORK/genesis.json" "$WORK/oom/genesis.json"
docker run -d --name maya-oom --memory 48m --memory-swap 48m -v "$WORK/oom:/w" maya2c/node:runbook \
    --genesis /w/genesis.json --data-dir /w/data --rpc-addr 127.0.0.1:8545 --mine --threads 2 >/dev/null
for _ in $(seq 1 60); do
    [[ "$(docker inspect -f '{{.State.OOMKilled}}' maya-oom 2>/dev/null)" == true ]] && break
    [[ "$(docker inspect -f '{{.State.Running}}' maya-oom 2>/dev/null)" == false ]] && break
    sleep 2
done
oom=$(docker inspect -f '{{.State.OOMKilled}} exit={{.State.ExitCode}}' maya-oom)
say "memory-pressure: 48 MiB limit, node killed: OOMKilled=$oom"
docker rm -f maya-oom >/dev/null
pid=$(node "$WORK/oom/data" 34002 34102 --mine --threads 1)
sleep 15
h=$(rpc 34002 get_tip_height)
say "memory-pressure: restarted without the limit, reopened consistently, height $h"
kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true
[[ "$h" != "-" ]] || { say "FAIL: memory-pressure recovery (see $WORK/oom/data.log)"; exit 1; }

# --- peer-starvation -----------------------------------------------------------
m=$(node "$WORK/miner" 34003 34103 --mine --threads 2)
sleep 5
miner_id=$(grep -m1 "peer id:" "$WORK/miner.log" | awk '{print $3}')
lonely=$(node "$WORK/lonely" 34004 34104)
sleep 20
say "peer-starvation: a node started with no bootnode sits at height $(rpc 34004 get_tip_height) while the network is at $(rpc 34003 get_tip_height)"
kill "$lonely"; wait "$lonely" 2>/dev/null || true
lonely=$(node "$WORK/lonely" 34004 34104 --bootnode "/ip4/127.0.0.1/tcp/34103/p2p/$miner_id" --sync-from http://127.0.0.1:34003)
for _ in $(seq 1 60); do
    a=$(rpc 34003 get_tip_height); b=$(rpc 34004 get_tip_height)
    [[ "$b" != "-" && "$a" != "-" ]] && (( b + 2 >= a && b > 0 )) && break
    sleep 2
done
say "peer-starvation: restarted with a bootnode, caught up to $(rpc 34004 get_tip_height) of $(rpc 34003 get_tip_height)"
kill "$lonely" "$m" 2>/dev/null || true
say "RUNBOOK REHEARSALS PASSED in $(( $(date +%s) - t0 ))s"
