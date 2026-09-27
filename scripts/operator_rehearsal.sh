#!/usr/bin/env bash
# Rehearses the MayaNetwork operator on a throwaway k3d cluster (Master Prompt
# 19 §1): install, snapshot restore, rolling upgrade — each timed and checked.
#
# Needs docker, k3d, kubectl, python3 with kopf + kubernetes, and a Linux
# maya2c-node binary (BIN, default target/ci/maya2c-node).
#
# The network is a four-replica proof-of-work devnet with one miner, because
# snapshot restore needs a chain a fresh node can fetch blocks of; a DAG-BFT
# node cannot yet catch up past the DAG's GC window (ADR-027). The operator
# itself does not care which consensus the image runs.
#
# "v2" is the same binary under a second tag: the rehearsal proves the
# rollout mechanics — one replica at a time, gated on the chain advancing —
# not a protocol upgrade (that is spec/ §4 and `upgrade_rehearsal.rs`).
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="${BIN:-target/ci/maya2c-node}"
CLUSTER=maya-operator
NS=maya-op
NET=devnet
LOG="${LOG:-reports/data/operator-rehearsal.txt}"
t0=$(date +%s)
say() { echo "[$(( $(date +%s) - t0 ))s] $*" | tee -a "$LOG"; }
: > "$LOG"

cleanup() {
    [[ -n "${OP_PID:-}" ]] && kill "$OP_PID" 2>/dev/null || true
    [[ "${KEEP:-0}" == 1 ]] || k3d cluster delete "$CLUSTER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

say "versions: $(k3d version | head -1); $(kubectl version --client | head -1); kopf $(python3 -c 'import kopf;print(kopf.__version__)')"
k3d cluster delete "$CLUSTER" >/dev/null 2>&1 || true
k3d cluster create "$CLUSTER" --agents 1 --wait --timeout 300s --k3s-arg "--disable=traefik@server:0" >/dev/null
say "cluster up"

CTX=$(mktemp -d); cp "$BIN" "$CTX/maya2c-node"
docker build -q -t maya2c/node:v1 -f infra/docker/Dockerfile.local "$CTX" >/dev/null
docker tag maya2c/node:v1 maya2c/node:v2
rm -rf "$CTX"
k3d image import maya2c/node:v1 maya2c/node:v2 -c "$CLUSTER" >/dev/null 2>&1
say "images v1, v2 imported"

kubectl create namespace "$NS" >/dev/null
cat > /tmp/genesis.json <<'EOF'
{"chain_id": "maya-operator-rehearsal", "timestamp": 1790000000, "difficulty_bits": 1,
 "pow_limit_bits": 1, "allocations": []}
EOF
kubectl -n "$NS" create configmap "$NET-genesis" --from-file=genesis.json=/tmp/genesis.json >/dev/null
kubectl apply -f infra/operator/crd.yaml >/dev/null
kubectl wait --for=condition=established crd/mayanetworks.maya2c.io --timeout=60s >/dev/null

python3 -m kopf run --standalone -n "$NS" infra/operator/maya_operator.py > /tmp/operator.log 2>&1 &
OP_PID=$!
sleep 5
say "operator running (pid $OP_PID)"

height() {
    kubectl -n "$NS" exec "$NET-$1" -- curl -sf -m 3 -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","id":1,"method":"get_tip_height","params":[]}' http://127.0.0.1:8545 2>/dev/null \
        | python3 -c 'import sys,json; print(json.load(sys.stdin)["result"])' 2>/dev/null || echo 0
}
wait_heights() {  # $1 = minimum height on every replica, $2 = timeout seconds
    local deadline=$(( $(date +%s) + $2 ))
    while :; do
        local lo=999999999
        for i in 0 1 2 3; do h=$(height "$i"); (( h < lo )) && lo=$h; done
        (( lo >= $1 )) && return 0
        (( $(date +%s) > deadline )) && { say "FAIL: heights did not reach $1 (lowest $lo)"; return 1; }
        sleep 5
    done
}
status() { kubectl -n "$NS" get mnet "$NET" -o jsonpath="{.status.$1}"; }
wait_phase() {
    local deadline=$(( $(date +%s) + $2 ))
    until [[ "$(status phase)" == "$1" ]]; do
        (( $(date +%s) > deadline )) && { say "FAIL: phase $1 not reached; operator log:"; tail -20 /tmp/operator.log | tee -a "$LOG"; return 1; }
        sleep 3
    done
}

# 1. Install.
cat <<EOF | kubectl apply -f - >/dev/null
apiVersion: maya2c.io/v1alpha1
kind: MayaNetwork
metadata: {name: $NET, namespace: $NS}
spec: {replicas: 4, image: "maya2c/node:v1", genesisConfigMap: "$NET-genesis", minerOrdinal: 0,
       snapshotInterval: 10, pruneDepth: 10, healthTimeoutSeconds: 600}
EOF
wait_phase Installed 120
kubectl -n "$NS" rollout status statefulset/"$NET" --timeout=600s >/dev/null
say "install: 4 replicas Ready"
wait_heights 25 900
say "install: every replica at height >= 25 ($(for i in 0 1 2 3; do printf '%s ' "$(height $i)"; done))"

# 2. Snapshot restore of replica 3.
before=$(height 3)
kubectl -n "$NS" patch mnet "$NET" --type merge -p '{"spec":{"restore":{"replica":3,"generation":1}}}' >/dev/null
wait_phase Restored 900
say "restore: replica 3 rebuilt from replica 0's snapshot (height before $before, now $(height 3)); $(status lastRestore)"
kubectl -n "$NS" logs "$NET-3" | grep -m1 "bootstrap:" | tee -a "$LOG" || true

# 3. Rolling upgrade v1 -> v2.
kubectl -n "$NS" patch mnet "$NET" --type merge -p '{"spec":{"image":"maya2c/node:v2"}}' >/dev/null
wait_phase Upgraded 1800
say "upgrade: $(status lastUpgrade)"
for i in 0 1 2 3; do
    say "  $NET-$i image $(kubectl -n "$NS" get pod "$NET-$i" -o jsonpath='{.spec.containers[0].image}') height $(height $i)"
done

# 4. Agreement after all of it.
h=$(( $(height 0) - 2 ))
ids=$(for i in 0 1 2 3; do kubectl -n "$NS" exec "$NET-$i" -- curl -sf -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"get_block_by_height\",\"params\":[$h]}" http://127.0.0.1:8545 \
    | python3 -c 'import sys,json; r=json.load(sys.stdin)["result"]; print(r.get("id") or r.get("hash"))'; done | sort -u | wc -l)
[[ "$ids" == 1 ]] && say "agreement: all 4 replicas hold the same block at height $h" || { say "FAIL: replicas disagree at $h"; exit 1; }
say "REHEARSAL PASSED in $(( $(date +%s) - t0 ))s"
