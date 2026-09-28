#!/usr/bin/env bash
# infra/testnet-vm/install.sh — a public Maya2C testnet on ONE Linux VM.
#
#   git clone https://github.com/EricWijesinghe/Maya2C && cd Maya2C
#   sudo DOMAIN=testnet.example.org EMAIL=you@example.org infra/testnet-vm/install.sh
#
# What it builds and runs, each as a sandboxed systemd service under a
# dedicated `maya` user:
#
#   maya2c-validator@0..3   four DAG-BFT validators (RPC on 127.0.0.1 only)
#   maya2c-gateway          REST/GraphQL, the only public HTTP surface
#   maya-chat-relay         Maya Chat store-and-forward relay (ADR-031)
#   caddy                   TLS for the gateway, when DOMAIN is set
#
# Four validators on one machine is a single-operator testnet: it has
# finality but not decentralisation. Other operators join through staking
# (LAUNCH.md Gate 2/3). This is a TESTNET: the chain id is refused if it names
# mainnet, and nothing here carries value.
#
# Keys are generated ON THIS MACHINE, readable only by `maya`, and never
# printed. The funded wallet's password is random and stored root-only.
# Nobody else, including whoever wrote this script, ever sees them.
#
# Re-running it is an upgrade: it rebuilds from the checkout it is run from and
# restarts the services, keeping keys, genesis and chain data.
#
# Environment:
#   DOMAIN      gateway hostname; with it, Caddy serves https://DOMAIN (80/443)
#   EMAIL       ACME contact for DOMAIN's certificate (recommended with DOMAIN)
#   CHAIN_ID    default maya2c-testnet-<UTC date of first install>
#   DRY_RUN=1   print every command that changes the system, run none of them
#
# Tested on: see infra/testnet-vm/README.md (the host it ran on and its log).

set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
TOOLCHAIN="nightly-2026-07-15"
PREFIX=/opt/maya2c
STATE=/var/lib/maya2c
CONF=/etc/maya2c
BIN=/usr/local/bin
VALIDATORS=4
RPC_BASE=32000
P2P_BASE=31100
GATEWAY_LOCAL=127.0.0.1:8080
CHAT_PORT=4001
GENESIS_BALANCE=10000000
BOND=100000

DRY_RUN="${DRY_RUN:-0}"
DOMAIN="${DOMAIN:-}"
EMAIL="${EMAIL:-}"

log() { printf '[testnet] %s\n' "$*"; }
die() { printf '[testnet] error: %s\n' "$*" >&2; exit 1; }
run() { if [ "$DRY_RUN" = 1 ]; then printf '[dry-run] %s\n' "$*"; else "$@"; fi; }
as_maya() { run runuser -u maya -- "$@"; }

preflight() {
    [ "$(uname -s)" = Linux ] || die "Linux only"
    [ "$DRY_RUN" = 1 ] || [ "$(id -u)" -eq 0 ] || die "run as root (sudo), or DRY_RUN=1"
    command -v apt-get >/dev/null || die "needs a Debian/Ubuntu host (apt-get)"
    command -v systemctl >/dev/null || die "needs systemd"
    [ -f "$REPO/Cargo.toml" ] || die "run from a Maya2C checkout"
    local mem_kb disk_kb
    mem_kb=$(awk '/MemTotal/ {print $2}' /proc/meminfo)
    [ "$mem_kb" -ge 3500000 ] || die "needs at least 4 GB RAM to build (has $((mem_kb / 1024)) MB)"
    disk_kb=$(df -Pk "$REPO" | awk 'NR==2 {print $4}')
    [ "$disk_kb" -ge 20000000 ] || die "needs at least 20 GB free for the build"
    if [ -n "$DOMAIN" ] && [ -z "$EMAIL" ]; then
        log "warning: DOMAIN without EMAIL; Let's Encrypt will not be able to reach you about the certificate"
    fi
}

install_deps() {
    log "installing build dependencies"
    run apt-get update -qq
    run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
        build-essential clang libclang-dev mold pkg-config libssl-dev git curl jq ufw ca-certificates
    if [ -n "$DOMAIN" ] && ! command -v caddy >/dev/null; then
        run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy
    fi
    if ! id maya >/dev/null 2>&1; then
        run useradd --system --home-dir "$STATE" --shell /usr/sbin/nologin maya
    fi
    run install -d -m 0755 "$PREFIX"
    run install -d -m 0750 -o maya -g maya "$STATE"
    run install -d -m 0750 -o root -g maya "$CONF"
}

build() {
    log "building the node (production features), wallet, gateway and chat (release; 20-60 minutes on a small VM)"
    export RUSTUP_HOME="$PREFIX/rustup" CARGO_HOME="$PREFIX/cargo"
    # RUSTUP_TOOLCHAIN overrides rust-toolchain.toml, which would otherwise
    # install miri, wasm and embedded targets a server never uses.
    export RUSTUP_TOOLCHAIN="$TOOLCHAIN"
    if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
        run bash -c "set -o pipefail; curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain $TOOLCHAIN"
    fi
    # An interrupted rustup install leaves a zero-byte `rustup` behind, and
    # every later cargo call then fails silently. Found on the first test run.
    if [ "$DRY_RUN" != 1 ] && ! "$CARGO_HOME/bin/cargo" --version >/dev/null 2>&1; then
        die "the Rust toolchain in $PREFIX is broken; remove $PREFIX/cargo and $PREFIX/rustup and run again"
    fi
    local cargo=("$CARGO_HOME/bin/cargo" build --release --locked --manifest-path "$REPO/Cargo.toml")
    # The node exactly as mainnet builds it (Production Standing Orders):
    # dag-bft only, no devnet mining flag. A testnet that runs a different
    # binary from mainnet tests the wrong thing.
    run "${cargo[@]}" -p maya2c-node --no-default-features --features production
    run "${cargo[@]}" -p l1-wallet -p maya-api-gateway -p maya-chat
    for b in maya2c-node l1-wallet maya2c-gateway maya-chat; do
        run install -m 0755 "$REPO/target/release/$b" "$BIN/$b"
    done
}

# Keys and genesis: only on first install. A second genesis would be a
# second chain, so an existing one is never touched.
genesis() {
    if [ -f "$CONF/genesis.json" ]; then
        log "genesis exists; keeping keys, genesis and chain data"
        return
    fi
    local chain_id="${CHAIN_ID:-maya2c-testnet-$(date -u +%Y%m%d)}"
    case "$chain_id" in *mainnet*) die "chain id '$chain_id' names mainnet; this installer is testnet only" ;; esac
    if [ "$DRY_RUN" = 1 ] && ! command -v jq >/dev/null; then
        log "dry run: jq not installed yet (the real run installs it); skipping the genesis preview"
        return
    fi
    log "generating $VALIDATORS validator keys and a funded wallet on this machine"
    local keys=() i
    for i in $(seq 0 $((VALIDATORS - 1))); do
        run install -d -m 0700 -o maya -g maya "$STATE/v$i"
        if [ "$DRY_RUN" = 1 ]; then keys+=("<v$i-public-key>"); continue; fi
        keys+=("$(runuser -u maya -- "$BIN/maya2c-node" --generate-validator-key "$STATE/v$i/validator.key" | tr -d '[:space:]')")
    done
    local password address
    if [ "$DRY_RUN" = 1 ]; then
        address="<wallet-address>"
    else
        umask 077
        password="$(head -c 32 /dev/urandom | base64 | tr -d '/+=' | head -c 40)"
        printf '%s' "$password" > "$CONF/wallet.password"
        address="$(L1_WALLET_PASSWORD="$password" "$BIN/l1-wallet" --keystore "$CONF/wallet.key" generate \
            | awk -F': *' '/address/ {print $2; exit}')"
        unset password
        [ -n "$address" ] || die "l1-wallet generate printed no address"
        chmod 0600 "$CONF/wallet.key" "$CONF/wallet.password"
    fi
    local validators bonds
    validators=$(printf '%s\n' "${keys[@]}" | jq -R . | jq -sc .)
    bonds=$(printf '%s\n' "${keys[@]}" | jq -R --arg a "$address" --argjson b "$BOND" '{operator: $a, bond: $b}' | jq -sc .)
    local doc
    doc=$(jq -n --arg id "$chain_id" --arg addr "$address" --argjson v "$validators" --argjson bonds "$bonds" \
        --argjson bal "$GENESIS_BALANCE" --argjson ts "$(date -u +%s)" '{
        chain_id: $id, timestamp: $ts, difficulty_bits: 0, pow_limit_bits: 0,
        allocations: [{address: $addr, balance: $bal}],
        bft: {validators: $v, anchor_timeout_ms: 1000, batch_size: 500,
              fees: {initial_base_fee: 1, min_base_fee: 1, target_block_bytes: 2621440, change_denominator: 8},
              staking: {epoch_blocks: 20, bonds: $bonds}}}')
    if [ "$DRY_RUN" = 1 ]; then printf '[dry-run] write %s:\n%s\n' "$CONF/genesis.json" "$doc"; return; fi
    printf '%s\n' "$doc" > "$CONF/genesis.json"
    chmod 0644 "$CONF/genesis.json"
}

units() {
    log "installing systemd units"
    local unit_dir="$REPO/infra/testnet-vm/systemd" u
    for u in maya2c-validator@.service maya2c-gateway.service maya-chat-relay.service; do
        run install -m 0644 "$unit_dir/$u" /etc/systemd/system/
    done
    local i p peers
    for i in $(seq 0 $((VALIDATORS - 1))); do
        peers=""
        for p in $(seq 0 $((VALIDATORS - 1))); do
            [ "$p" = "$i" ] || peers="$peers --bootnode /ip4/127.0.0.1/tcp/$((P2P_BASE + p))"
        done
        if [ "$DRY_RUN" = 1 ]; then
            printf '[dry-run] write %s/v%s.env (rpc 127.0.0.1:%s, p2p %s)
' "$CONF" "$i" "$((RPC_BASE + i))" "$((P2P_BASE + i))"
        else
            printf 'RPC_ADDR=127.0.0.1:%s
P2P_PORT=%s
BOOTNODES=%s
' "$((RPC_BASE + i))" "$((P2P_BASE + i))" "${peers# }" > "$CONF/v$i.env"
        fi
    done
    run install -d -m 0700 -o maya -g maya "$STATE/relay"
    local listen="0.0.0.0:8080"
    [ -n "$DOMAIN" ] && listen="$GATEWAY_LOCAL"
    if [ "$DRY_RUN" = 1 ]; then
        printf '[dry-run] write %s/gateway.env with MAYA_GATEWAY_LISTEN=%s\n' "$CONF" "$listen"
    else
        printf 'MAYA_NODE_RPC=http://127.0.0.1:%s\nMAYA_GATEWAY_LISTEN=%s\n' "$RPC_BASE" "$listen" > "$CONF/gateway.env"
    fi
    run systemctl daemon-reload
}

firewall() {
    log "firewall: default deny; ssh, validator p2p, chat and the gateway open; node RPC stays on loopback"
    run ufw --force default deny incoming
    run ufw --force default allow outgoing
    # SSH first: a default-deny applied before the SSH rule loses the host.
    run ufw allow 22/tcp
    run ufw allow "$P2P_BASE/tcp" comment 'maya2c p2p (validator 0: bootnode for observers)'
    run ufw allow "$CHAT_PORT/tcp" comment 'maya-chat relay'
    if [ -n "$DOMAIN" ]; then
        run ufw allow 80/tcp comment 'acme http-01'
        run ufw allow 443/tcp comment 'gateway https'
    else
        run ufw allow 8080/tcp comment 'gateway http (no DOMAIN: plain HTTP)'
    fi
    run ufw --force enable
}

tls() {
    [ -n "$DOMAIN" ] || { log "no DOMAIN: the gateway is plain HTTP on :8080. Set DOMAIN for HTTPS."; return; }
    log "Caddy: https://$DOMAIN -> $GATEWAY_LOCAL"
    local site
    site="${EMAIL:+{
    email $EMAIL
\}
}$DOMAIN {
    encode zstd gzip
    reverse_proxy $GATEWAY_LOCAL
}"
    if [ "$DRY_RUN" = 1 ]; then printf '[dry-run] write /etc/caddy/Caddyfile:\n%s\n' "$site"; else printf '%s\n' "$site" > /etc/caddy/Caddyfile; fi
    run systemctl enable caddy
    run systemctl restart caddy
}

start() {
    log "starting services"
    local i
    for i in $(seq 0 $((VALIDATORS - 1))); do run systemctl enable "maya2c-validator@$i"; run systemctl restart "maya2c-validator@$i"; done
    run systemctl enable maya2c-gateway maya-chat-relay
    run systemctl restart maya2c-gateway maya-chat-relay
}

rpc() {
    curl -fsS --max-time 5 -H 'content-type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":$3}" "http://127.0.0.1:$(($RPC_BASE + $1))/"
}

# Every validator returns the same block id at a height all have reached.
verify() {
    [ "$DRY_RUN" = 1 ] && { log "dry run: skipping the live checks"; return; }
    log "waiting for the validators to agree (up to 3 minutes)"
    local deadline=$((SECONDS + 180)) h ids
    while [ $SECONDS -lt $deadline ]; do
        h=$(rpc 0 get_block_by_height '[3]' 2>/dev/null | jq -r '.result.header.id // empty' || true)
        if [ -n "$h" ]; then
            ids=$(for i in $(seq 0 $((VALIDATORS - 1))); do rpc "$i" get_block_by_height '[3]' | jq -r '.result.header.id // empty'; done | sort -u)
            if [ "$(printf '%s\n' "$ids" | grep -c .)" = 1 ]; then
                log "all $VALIDATORS validators agree on block 3: $ids"
                curl -fsS --max-time 5 -o /dev/null "http://$GATEWAY_LOCAL/health" 2>/dev/null \
                    || curl -fsS --max-time 5 -o /dev/null "http://127.0.0.1:8080/health" \
                    || die "the gateway does not answer /health (journalctl -u maya2c-gateway)"
                log "gateway healthy"
                return
            fi
        fi
        sleep 3
    done
    die "the validators did not agree within 3 minutes (journalctl -u 'maya2c-validator@*')"
}

summary() {
    [ "$DRY_RUN" = 1 ] && return
    local ip relay
    ip=$(curl -fsS --max-time 5 https://api.ipify.org 2>/dev/null || hostname -I | awk '{print $1}')
    relay=$(journalctl -u maya-chat-relay --no-pager -o cat | awk '/relay listening on \/ip4\/0\.0\.0\.0/ {print $4}' | tail -1)
    cat <<EOF

[testnet] UP — $(jq -r .chain_id "$CONF/genesis.json")
  gateway      ${DOMAIN:+https://$DOMAIN}${DOMAIN:-http://$ip:8080}
  chat relay   ${relay/0.0.0.0/$ip}
  p2p (join)   /ip4/$ip/tcp/$P2P_BASE
  genesis      $CONF/genesis.json   sha256 $(sha256sum "$CONF/genesis.json" | cut -c1-64)
  wallet       $CONF/wallet.key (root only; password in $CONF/wallet.password)
  logs         journalctl -u 'maya2c-*' -u maya-chat-relay -f
Publish the genesis file and its sha256 so others can verify they joined this chain.
EOF
}

preflight
install_deps
build
genesis
units
firewall
tls
start
verify
summary
