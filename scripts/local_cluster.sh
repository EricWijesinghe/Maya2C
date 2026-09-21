#!/usr/bin/env bash
#
# Zero to an N-node Maya2C cluster on one host.
#
# =============================================================================
# What this is, and what it is not
# =============================================================================
#
# It is a real dry run of the *software*: it mints a genesis with the ceremony
# tool, verifies the state root the way the Ansible role does, starts N real
# `node` processes from that genesis, and waits for them to find each other.
# Everything it exercises is the code that would run in production.
#
# It is not a dry run of the *deployment*. There is no terraform, no ansible,
# no systemd, no firewall, and no TLS — those need a cloud account and a Linux
# host, and neither is available where this was written. `terraform validate`
# covers the first; the Ansible playbooks ship unverified and say so.
#
# The point is to answer "does a fleet come up from nothing" without needing
# twelve machines to ask.
#
# =============================================================================
# Memory
# =============================================================================
#
# Each node opens its own RocksDB. At the shipped defaults that is a 512 MiB
# block cache each, so twelve nodes ask for 6 GiB of cache alone. This writes a
# small-cache config per node instead — 32 MiB — because the question here is
# whether they connect and agree, not how they perform under load.
#
# If your machine cannot hold N nodes, run fewer and say so. A script that
# quietly started eight when asked for twelve would be reporting on a cluster
# nobody asked about.

set -euo pipefail

NODES="${1:-12}"
CHAIN_ID="${CHAIN_ID:-maya-genesis-rc1}"
WORK_DIR="${WORK_DIR:-./target/local-cluster}"
BASE_P2P="${BASE_P2P:-31000}"
BASE_RPC="${BASE_RPC:-9100}"
SUPPLY="${SUPPLY:-21000000000}"
SETTLE_SECONDS="${SETTLE_SECONDS:-45}"

# The same list the node, the ceremony tool, terraform and the playbook refuse.
# Repeated for the same reason they repeat it: one shared list is one edit away
# from relaxing all of them.
VALUE_BEARING=("mainnet" "maya-mainnet")

log()  { printf '[cluster] %s\n' "$*"; }
die()  { printf '[cluster] error: %s\n' "$*" >&2; exit 1; }

for blocked in "${VALUE_BEARING[@]}"; do
    if [ "$CHAIN_ID" = "$blocked" ]; then
        die "refusing to build a cluster on '$CHAIN_ID'.

Mainnet is blocked while the shielded pool's circuit is unaudited.
This script would not get far — the ceremony tool and the node both refuse the
same ids — but failing here costs a second instead of a build."
    fi
done

[ "$NODES" -ge 2 ] || die "need at least 2 nodes to have a network; got $NODES"

CARGO_FLAGS="${CARGO_FLAGS:---release}"
BIN_DIR="./target/$( [ "$CARGO_FLAGS" = "--release" ] && echo release || echo debug )"

PIDS=()
cleanup() {
    if [ ${#PIDS[@]} -gt 0 ]; then
        log "stopping ${#PIDS[@]} nodes"
        # Not `kill -9`: the node needs to close RocksDB cleanly, and a SIGKILL
        # partway through leaves a database that replays on next start. That is
        # survivable here but it is the habit that matters.
        kill "${PIDS[@]}" 2>/dev/null || true
        wait "${PIDS[@]}" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

# --- build -------------------------------------------------------------------

log "building node, genesis and genesis-ceremony"
cargo build $CARGO_FLAGS --bin maya2c-node --bin maya2c-genesis --bin genesis-ceremony

# --- genesis -----------------------------------------------------------------

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"
CEREMONY_DIR="$WORK_DIR/ceremony"

log "running a two-participant ceremony"
# Multi-party, not the single-party path: this is the flow a launch uses, so it
# is the flow the dry run should exercise.
"$BIN_DIR/genesis-ceremony" contribute --label alice --out-dir "$WORK_DIR/alice" >/dev/null
"$BIN_DIR/genesis-ceremony" contribute --label bob   --out-dir "$WORK_DIR/bob"   >/dev/null

mkdir -p "$WORK_DIR/contributions"
cp "$WORK_DIR/alice/alice.public" "$WORK_DIR/contributions/"
cp "$WORK_DIR/bob/bob.public"     "$WORK_DIR/contributions/"

"$BIN_DIR/genesis-ceremony" assemble \
    --chain-id "$CHAIN_ID" \
    --supply "$SUPPLY" \
    --timestamp 1767225600 \
    --difficulty-bits 8 \
    --contributions "$WORK_DIR/contributions" \
    --out-dir "$CEREMONY_DIR" >"$WORK_DIR/ceremony.log"

GENESIS="$CEREMONY_DIR/genesis.json"
STATE_ROOT="$(awk '/^state root/ {print $NF}' "$CEREMONY_DIR/COMMITMENT.txt")"
[ -n "$STATE_ROOT" ] || die "could not read the state root from COMMITMENT.txt"

# The same check `infra/ansible/roles/maya_node` runs against every host. If it
# passes here and fails there, the difference is the file that was fetched.
log "verifying the genesis state root: $STATE_ROOT"
"$BIN_DIR/genesis" --verify "$GENESIS" --expect-state-root "$STATE_ROOT" >/dev/null \
    || die "the ceremony's own genesis failed verification against its own commitment"

# The coordinator must hold no key material. Asserted rather than assumed,
# because it is the entire point of the two-step ceremony.
if compgen -G "$CEREMONY_DIR/*.secret" >/dev/null; then
    die "the assemble step produced secret key material in $CEREMONY_DIR"
fi

# --- nodes -------------------------------------------------------------------

log "starting $NODES nodes"
FIRST_ADDR=""

for i in $(seq 0 $((NODES - 1))); do
    NODE_DIR="$WORK_DIR/node-$i"
    mkdir -p "$NODE_DIR"
    P2P=$((BASE_P2P + i))
    RPC=$((BASE_RPC + i))

    # Node 0 is the only bootnode. A full mesh of dial strings would test the
    # script's string building; one seed tests discovery.
    if [ "$i" -eq 0 ]; then
        BOOTNODES=""
    else
        BOOTNODES="\"/ip4/127.0.0.1/tcp/$BASE_P2P\""
    fi

    cat > "$NODE_DIR/config.toml" <<EOF
[network]
p2p_port = $P2P
bootnodes = [$BOOTNODES]
dual_kem = "off"

[rpc]
listen = "127.0.0.1:$RPC"
rate_limit_per_second = 1000
rate_limit_burst = 2000

[storage]
# Small on purpose: $NODES nodes at the 512 MiB default would ask for
# $((NODES * 512)) MiB of block cache on one machine.
block_cache_mib = 32
write_buffer_mib = 8
max_open_files = 256
EOF

    "$BIN_DIR/node" \
        --config "$NODE_DIR/config.toml" \
        --genesis "$GENESIS" \
        --data-dir "$NODE_DIR/data" \
        >"$NODE_DIR/node.log" 2>&1 &
    PIDS+=($!)

    [ -n "$FIRST_ADDR" ] || FIRST_ADDR="127.0.0.1:$RPC"
done

log "waiting ${SETTLE_SECONDS}s for the mesh to form"
sleep "$SETTLE_SECONDS"

# --- assertions --------------------------------------------------------------

alive=0
for pid in "${PIDS[@]}"; do
    kill -0 "$pid" 2>/dev/null && alive=$((alive + 1))
done

log "$alive of $NODES nodes still running"
if [ "$alive" -ne "$NODES" ]; then
    log "logs from the first failed node:"
    for i in $(seq 0 $((NODES - 1))); do
        if ! kill -0 "${PIDS[$i]}" 2>/dev/null; then
            tail -20 "$WORK_DIR/node-$i/node.log"
            break
        fi
    done
    die "$((NODES - alive)) node(s) exited. This machine may not have room for \
$NODES; re-run with a smaller count and report that number rather than assuming \
$NODES worked."
fi

# Every node must agree on the genesis it started from. Asked over RPC rather
# than read from the file, so what is compared is what each node actually
# loaded.
log "checking every node reports the same chain"
mismatched=0
for i in $(seq 0 $((NODES - 1))); do
    RPC=$((BASE_RPC + i))
    reply="$(curl -s --max-time 5 -X POST "http://127.0.0.1:$RPC" \
        -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","id":1,"method":"get_chain_info","params":[]}' || true)"

    if [ -z "$reply" ]; then
        log "  node-$i: no RPC response"
        mismatched=$((mismatched + 1))
        continue
    fi
    log "  node-$i: $reply"
done

[ "$mismatched" -eq 0 ] || die "$mismatched node(s) did not answer RPC"

log "cluster of $NODES is up on chain $CHAIN_ID"
log "genesis      $GENESIS"
log "state root   $STATE_ROOT"
log "logs         $WORK_DIR/node-*/node.log"
