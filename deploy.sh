#!/usr/bin/env bash
#
# Launch and verify a three-node L1 testnet.
#
# Steps:
#   1. Generate genesis.json once, if absent.
#   2. Build the image and start the fleet.
#   3. Wait for every node's RPC to answer.
#   4. Verify all three agree on genesis — that they are one network, not three.
#   5. Watch the chain grow and confirm the peers follow the seed.
#
# Usage:
#   ./deploy.sh                 launch and verify
#   ./deploy.sh --down          stop and remove containers (volumes preserved)
#   ./deploy.sh --destroy       stop and remove containers AND volumes
#   ./deploy.sh --status        report current heights without changing anything

set -euo pipefail

# --- configuration ---------------------------------------------------------

CHAIN_ID="${CHAIN_ID:-l1-testnet-1}"
# Pinned, not `date +%s`: a floating timestamp changes the genesis hash on every
# regeneration, which would silently fork the fleet.
GENESIS_TIMESTAMP="${GENESIS_TIMESTAMP:-1756252800}"
# Low enough that a small fleet produces blocks in seconds. Raise substantially
# for anything resembling a real network.
DIFFICULTY_BITS="${DIFFICULTY_BITS:-14}"
GENESIS_FILE="${GENESIS_FILE:-genesis.json}"

# Premined allocations, as <hex-address>:<amount>.
ALLOCATIONS=(
  "1111111111111111111111111111111111111111111111111111111111111111:1000000000"
  "2222222222222222222222222222222222222222222222222222222222222222:500000000"
)

# Host-side RPC ports, matching infra/docker/docker-compose.yml.
declare -a NODE_NAMES=("seed-node" "peer-1" "peer-2")
declare -a NODE_PORTS=(8545 8546 8547)

RPC_READY_TIMEOUT="${RPC_READY_TIMEOUT:-180}"
SYNC_TIMEOUT="${SYNC_TIMEOUT:-300}"
# Height every node must reach before the deployment is considered good.
TARGET_HEIGHT="${TARGET_HEIGHT:-3}"

# --- helpers ---------------------------------------------------------------

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m  ok\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m  !!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31mERROR\033[0m %s\n' "$*" >&2; exit 1; }

require() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required but not installed"
}

# Docker Compose ships as a plugin (`docker compose`) or a standalone binary
# (`docker-compose`) depending on age; support both rather than assuming.
compose() {
  if docker compose version >/dev/null 2>&1; then
    docker compose "$@"
  elif command -v docker-compose >/dev/null 2>&1; then
    docker-compose "$@"
  else
    die "neither 'docker compose' nor 'docker-compose' is available"
  fi
}

# Extracts a field from JSON. Prefers jq; falls back to python3 so the script
# does not hard-require jq on a freshly provisioned VPS.
json_field() {
  local json="$1" path="$2"
  if command -v jq >/dev/null 2>&1; then
    printf '%s' "$json" | jq -r "$path // empty"
  elif command -v python3 >/dev/null 2>&1; then
    printf '%s' "$json" | python3 -c '
import json, sys
path = sys.argv[1].lstrip(".").split(".")
try:
    value = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for key in path:
    if not isinstance(value, dict) or key not in value:
        sys.exit(0)
    value = value[key]
print(value)
' "$path"
  else
    die "either jq or python3 is required to parse RPC responses"
  fi
}

# Issues a JSON-RPC call. Returns non-zero if the node does not answer.
rpc() {
  local port="$1" method="$2" params="${3:-[]}"
  curl -fsS --max-time 10 \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}" \
    "http://127.0.0.1:${port}" 2>/dev/null
}

# Current height, or empty if the node is not answering.
node_height() {
  local port="$1" response
  response="$(rpc "$port" get_mining_candidate)" || return 1
  local next
  next="$(json_field "$response" ".result.height")"
  [ -n "$next" ] || return 1
  # get_mining_candidate reports the height of the *next* block.
  echo $(( next - 1 ))
}

node_genesis_id() {
  local port="$1" response
  response="$(rpc "$port" get_block_by_height "[0]")" || return 1
  json_field "$response" ".result.header.id"
}

# --- steps -----------------------------------------------------------------

generate_genesis() {
  if [ -f "$GENESIS_FILE" ]; then
    ok "$GENESIS_FILE already exists, reusing it"
    warn "delete it only if you intend to start a NEW network — every node's"
    warn "state directory is tied to this genesis and must be wiped alongside it"
    return
  fi

  log "generating $GENESIS_FILE"

  local alloc_args=()
  for allocation in "${ALLOCATIONS[@]}"; do
    alloc_args+=(--alloc "$allocation")
  done

  # Build the generator inside the same image, so genesis is produced by
  # exactly the binary the nodes run rather than by a host toolchain that may
  # differ.
  compose build seed-node >/dev/null
  docker run --rm --entrypoint /usr/local/bin/genesis \
    -v "$(pwd):/out" -w /out \
    custom-l1-node:latest \
    --chain-id "$CHAIN_ID" \
    --timestamp "$GENESIS_TIMESTAMP" \
    --difficulty-bits "$DIFFICULTY_BITS" \
    "${alloc_args[@]}" \
    --out "$GENESIS_FILE"

  ok "wrote $GENESIS_FILE"
}

start_fleet() {
  log "building images and starting the fleet"
  compose up -d --build
  ok "containers started"
}

wait_for_rpc() {
  log "waiting for RPC on all ${#NODE_NAMES[@]} nodes (timeout ${RPC_READY_TIMEOUT}s)"

  local deadline=$(( SECONDS + RPC_READY_TIMEOUT ))
  local index
  for index in "${!NODE_NAMES[@]}"; do
    local name="${NODE_NAMES[$index]}" port="${NODE_PORTS[$index]}"
    while :; do
      if node_height "$port" >/dev/null 2>&1; then
        ok "$name is answering on port $port"
        break
      fi
      if [ "$SECONDS" -ge "$deadline" ]; then
        warn "recent logs from $name:"
        compose logs --tail 40 "$name" >&2 || true
        die "$name did not answer RPC within ${RPC_READY_TIMEOUT}s"
      fi
      sleep 2
    done
  done
}

# Every node must derive the same genesis. Differing genesis hashes mean the
# nodes are on separate networks and will never converge, no matter how long
# they run — so this is checked before waiting on sync.
verify_same_network() {
  log "verifying all nodes share one genesis"

  local reference="" index
  for index in "${!NODE_NAMES[@]}"; do
    local name="${NODE_NAMES[$index]}" port="${NODE_PORTS[$index]}"
    local id
    id="$(node_genesis_id "$port")" || die "could not read genesis from $name"
    [ -n "$id" ] || die "$name returned an empty genesis id"

    if [ -z "$reference" ]; then
      reference="$id"
      ok "genesis ${id:0:16}… (from $name)"
    elif [ "$id" != "$reference" ]; then
      die "$name has genesis ${id:0:16}… but seed-node has ${reference:0:16}… — these are different networks; wipe volumes with --destroy and redeploy"
    else
      ok "$name agrees"
    fi
  done
}

verify_sync() {
  log "waiting for all nodes to reach height >= ${TARGET_HEIGHT} (timeout ${SYNC_TIMEOUT}s)"

  local deadline=$(( SECONDS + SYNC_TIMEOUT ))
  while :; do
    local all_synced=1 report="" index
    for index in "${!NODE_NAMES[@]}"; do
      local name="${NODE_NAMES[$index]}" port="${NODE_PORTS[$index]}"
      local height
      height="$(node_height "$port" 2>/dev/null || echo "")"
      if [ -z "$height" ]; then
        height="?"
        all_synced=0
      elif [ "$height" -lt "$TARGET_HEIGHT" ]; then
        all_synced=0
      fi
      report+="${name}=${height} "
    done

    printf '\r  %s' "$report"

    if [ "$all_synced" -eq 1 ]; then
      printf '\n'
      ok "all nodes reached height >= ${TARGET_HEIGHT}"
      return
    fi

    if [ "$SECONDS" -ge "$deadline" ]; then
      printf '\n'
      warn "final heights: $report"
      warn "peer-2 does not mine — if only it is behind, block propagation is the problem"
      compose logs --tail 40 >&2 || true
      die "nodes did not synchronize within ${SYNC_TIMEOUT}s"
    fi

    sleep 3
  done
}

show_status() {
  log "fleet status"
  local index
  for index in "${!NODE_NAMES[@]}"; do
    local name="${NODE_NAMES[$index]}" port="${NODE_PORTS[$index]}"
    local height genesis
    height="$(node_height "$port" 2>/dev/null || echo "unreachable")"
    genesis="$(node_genesis_id "$port" 2>/dev/null || echo "")"
    printf '  %-12s rpc=127.0.0.1:%-5s height=%-8s genesis=%s\n' \
      "$name" "$port" "$height" "${genesis:0:16}"
  done
}

summary() {
  log "testnet is up"
  show_status
  cat <<'EOF'

  RPC endpoints:
    seed-node  http://127.0.0.1:8545
    peer-1     http://127.0.0.1:8546
    peer-2     http://127.0.0.1:8547

  Try:
    curl -s -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"get_block_by_height","params":[1]}' \
      http://127.0.0.1:8545

    ./deploy.sh --status     current heights
    ./deploy.sh --down       stop, keep chain data
    ./deploy.sh --destroy    stop and wipe chain data
EOF
}

# --- entry point -----------------------------------------------------------

main() {
  case "${1:-up}" in
    --down)
      log "stopping the fleet (volumes preserved)"
      compose down
      ok "stopped"
      ;;
    --destroy)
      log "stopping the fleet and deleting all chain data"
      compose down -v
      ok "containers and volumes removed"
      warn "$GENESIS_FILE was left in place; delete it too to start a new network"
      ;;
    --status)
      require curl
      show_status
      ;;
    up|"")
      require docker
      require curl
      generate_genesis
      start_fleet
      wait_for_rpc
      verify_same_network
      verify_sync
      summary
      ;;
    -h|--help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      ;;
    *)
      die "unknown argument: $1 (try --help)"
      ;;
  esac
}

main "$@"
