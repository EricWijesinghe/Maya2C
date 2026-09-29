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
#   maya2c-faucet           rate-limited test coins (seed mode only)
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
#   VALIDATORS  seed mode: validators on this VM, 1-4 (default 4)
#   DRY_RUN=1   print every command that changes the system, run none of them
#
# Join mode (another machine joining a running seed network, e.g. a home PC):
#   GENESIS_URL      where the seed operator published genesis.json
#   EXPECTED_SHA256  the sha256 the seed operator published with it
#   BOOTNODE         the seed's p2p address, /ip4/<ip>/tcp/31100
# A joining node starts as an observer; it enters the committee through a
# staking registration, and the installer prints the public key to register.
#
# Tested on: see infra/testnet-vm/README.md (the host it ran on and its log).

set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
TOOLCHAIN="nightly-2026-07-15"
PREFIX=/opt/maya2c
STATE=/var/lib/maya2c
CONF=/etc/maya2c
BIN=/usr/local/bin
VALIDATORS="${VALIDATORS:-4}"
RPC_BASE=32000
P2P_BASE=31100
GATEWAY_LOCAL=127.0.0.1:8080
CHAT_PORT=4001
GENESIS_BALANCE=10000000
FAUCET_BALANCE=5000000
FAUCET_LOCAL=127.0.0.1:8090
FAUCET_PORT=8090
BOND=100000

DRY_RUN="${DRY_RUN:-0}"
DOMAIN="${DOMAIN:-}"
EMAIL="${EMAIL:-}"
GENESIS_URL="${GENESIS_URL:-}"
EXPECTED_SHA256="${EXPECTED_SHA256:-}"
BOOTNODE="${BOOTNODE:-}"

log() { printf '[testnet] %s\n' "$*"; }
die() { printf '[testnet] error: %s\n' "$*" >&2; exit 1; }
run() { if [ "$DRY_RUN" = 1 ]; then printf '[dry-run] %s\n' "$*"; else "$@"; fi; }
as_maya() { run runuser -u maya -- "$@"; }
# A fresh cloud VM runs unattended-upgrades at boot, which holds the dpkg lock
# for minutes; without waiting, apt-get fails at once (found on the test VM).
APT=(apt-get -o DPkg::Lock::Timeout=600)

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
    case "$VALIDATORS" in [1-4]) ;; *) die "VALIDATORS must be 1-4 (got '$VALIDATORS')" ;; esac
    if [ -n "$GENESIS_URL" ]; then
        [ -n "$EXPECTED_SHA256" ] || die "join mode needs EXPECTED_SHA256, published by the seed operator"
        [ -n "$BOOTNODE" ] || die "join mode needs BOOTNODE, the seed's /ip4/<ip>/tcp/31100"
        VALIDATORS=1
    fi
    if [ -n "$DOMAIN" ] && [ -z "$EMAIL" ]; then
        log "warning: DOMAIN without EMAIL; Let's Encrypt will not be able to reach you about the certificate"
    fi
}

install_deps() {
    log "installing build dependencies"
    run "${APT[@]}" update -qq
    run env DEBIAN_FRONTEND=noninteractive "${APT[@]}" install -y -qq \
        build-essential clang libclang-dev mold pkg-config libssl-dev git curl jq ca-certificates
    if [ -n "$DOMAIN" ] && ! command -v caddy >/dev/null; then
        run env DEBIAN_FRONTEND=noninteractive "${APT[@]}" install -y -qq caddy
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
    run "${cargo[@]}" -p l1-wallet -p maya-api-gateway -p maya-chat -p maya-faucet
    for b in maya2c-node l1-wallet maya2c-gateway maya-chat maya-faucet; do
        run install -m 0755 "$REPO/target/release/$b" "$BIN/$b"
    done
}

# Join mode: fetch the seed network's genesis and refuse it unless its sha256
# is the one the seed operator published. The hash is what tells an operator
# they joined the network they meant to; a wrong file is a different chain.
join_genesis() {
    log "join mode: fetching genesis from $GENESIS_URL"
    local tmp actual pub
    tmp="$(mktemp)"
    run curl -fsSL --max-time 60 -o "$tmp" "$GENESIS_URL"
    if [ "$DRY_RUN" != 1 ]; then
        actual="$(sha256sum "$tmp" | cut -c1-64)"
        [ "$actual" = "$EXPECTED_SHA256" ] || die "genesis sha256 mismatch.
  expected  $EXPECTED_SHA256
  fetched   $actual
This is not the genesis the seed operator published. Do not proceed."
        log "genesis sha256 verified: $actual"
    fi
    run install -m 0644 "$tmp" "$CONF/genesis.json"
    rm -f "$tmp"
    run install -d -m 0700 -o maya -g maya "$STATE/v0"
    if [ "$DRY_RUN" = 1 ]; then return; fi
    # A key outside the genesis committee runs as an observer until a
    # staking registration puts it in the committee (bft_staking_tests).
    pub="$(runuser -u maya -- "$BIN/maya2c-node" --generate-validator-key "$STATE/v0/validator.key" | tr -d '[:space:]')"
    printf '%s\n' "$pub" > "$CONF/v0.pub"
}

# Keys and genesis: only on first install. A second genesis would be a
# second chain, so an existing one is never touched.
genesis() {
    if [ -f "$CONF/genesis.json" ]; then
        log "genesis exists; keeping keys, genesis and chain data"
        return
    fi
    if [ -n "$GENESIS_URL" ]; then
        join_genesis
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
    # The faucet's key: generated here, readable by root only, loaded by
    # systemd into the faucet's environment. Only its address is printed.
    local faucet
    if [ "$DRY_RUN" = 1 ]; then
        faucet="<faucet-address>"
    else
        faucet="$("$BIN/maya-faucet" generate-key "$CONF/faucet.env" | tr -d '[:space:]')"
        [ -n "$faucet" ] || die "maya-faucet generate-key printed no address"
    fi
    local validators bonds
    validators=$(printf '%s\n' "${keys[@]}" | jq -R . | jq -sc .)
    bonds=$(printf '%s\n' "${keys[@]}" | jq -R --arg a "$address" --argjson b "$BOND" '{operator: $a, bond: $b}' | jq -sc .)
    local doc
    doc=$(jq -n --arg id "$chain_id" --arg addr "$address" --argjson v "$validators" --argjson bonds "$bonds" \
        --argjson bal "$GENESIS_BALANCE" --arg faucet "$faucet" --argjson fbal "$FAUCET_BALANCE" \
        --argjson ts "$(date -u +%s)" '{
        chain_id: $id, timestamp: $ts, difficulty_bits: 0, pow_limit_bits: 0,
        allocations: [{address: $addr, balance: $bal}, {address: $faucet, balance: $fbal}],
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
    for u in maya2c-validator@.service maya2c-gateway.service maya-chat-relay.service maya2c-faucet.service; do
        run install -m 0644 "$unit_dir/$u" /etc/systemd/system/
    done
    local i p peers
    for i in $(seq 0 $((VALIDATORS - 1))); do
        # Seed mode: every local validator peers with the others. Join mode:
        # the one local node dials the seed network's bootnode.
        peers=""
        if [ -n "$BOOTNODE" ]; then
            peers="--bootnode $BOOTNODE"
        else
            for p in $(seq 0 $((VALIDATORS - 1))); do
                [ "$p" = "$i" ] || peers="$peers --bootnode /ip4/127.0.0.1/tcp/$((P2P_BASE + p))"
            done
        fi
        if [ "$DRY_RUN" = 1 ]; then
            printf '[dry-run] write %s/v%s.env (rpc 127.0.0.1:%s, p2p %s,%s)\n' \
                "$CONF" "$i" "$((RPC_BASE + i))" "$((P2P_BASE + i))" "${peers:- no peers}"
        else
            printf 'RPC_ADDR=127.0.0.1:%s\nP2P_PORT=%s\nBOOTNODES=%s\n' \
                "$((RPC_BASE + i))" "$((P2P_BASE + i))" "${peers# }" > "$CONF/v$i.env"
        fi
    done
    run install -d -m 0700 -o maya -g maya "$STATE/relay"
    # The faucet's grant journal: without it a restart resets every limit.
    run install -d -m 0700 -o maya -g maya "$STATE/faucet"
    local listen="0.0.0.0:8080"
    [ -n "$DOMAIN" ] && listen="$GATEWAY_LOCAL"
    if [ "$DRY_RUN" = 1 ]; then
        printf '[dry-run] write %s/gateway.env with MAYA_GATEWAY_LISTEN=%s\n' "$CONF" "$listen"
    else
        printf 'MAYA_NODE_RPC=http://127.0.0.1:%s\nMAYA_GATEWAY_LISTEN=%s\n' "$RPC_BASE" "$listen" > "$CONF/gateway.env"
    fi
    # Behind Caddy the faucet sees the proxy's address, so it keys its limits
    # on the X-Forwarded-For Caddy sets; exposed directly, it must not trust
    # that header, which any caller can forge.
    local faucet_listen="0.0.0.0:$FAUCET_PORT" trust=false
    [ -n "$DOMAIN" ] && { faucet_listen="$FAUCET_LOCAL"; trust=true; }
    if [ "$DRY_RUN" = 1 ]; then
        printf '[dry-run] write %s/faucet-config.env (listen %s, trust proxy %s)\n' "$CONF" "$faucet_listen" "$trust"
    elif [ -f "$CONF/genesis.json" ]; then
        # CORS: the "Join the testnet" page on maya2c.dev calls the faucet.
        printf 'MAYA_FAUCET_CHAIN=%s\nMAYA_FAUCET_NODE=http://127.0.0.1:%s\nMAYA_FAUCET_LISTEN=%s\nMAYA_FAUCET_TRUST_PROXY=%s\nMAYA_FAUCET_LEDGER=%s\nMAYA_FAUCET_CORS_ORIGINS=https://maya2c.dev\n' \
            "$(jq -r .chain_id "$CONF/genesis.json")" "$RPC_BASE" "$faucet_listen" "$trust" \
            "$STATE/faucet/ledger.jsonl" > "$CONF/faucet-config.env"
    fi
    run systemctl daemon-reload
}

# The firewall is plain iptables in its own chain, not ufw. ufw appends its
# rules, and Oracle's Ubuntu images end INPUT with a REJECT loaded at boot
# by netfilter-persistent, so every `ufw allow` landed below it and did
# nothing. ufw and iptables-persistent also conflict as packages. Our chain
# is jumped to from the TOP of INPUT, so it works whether or not the image
# ships its own REJECT, and netfilter-persistent reloads it at boot.
fw_ports() {
    printf '%s
' "$P2P_BASE" "$CHAT_PORT"
    if [ -n "$DOMAIN" ]; then printf '80\n443\n'; else printf '8080\n%s\n' "$FAUCET_PORT"; fi
}

fw_chain() { # $1 = iptables or ip6tables
    local t="$1" port
    run "$t" -N MAYA2C-IN 2>/dev/null || true
    run "$t" -F MAYA2C-IN
    run "$t" -A MAYA2C-IN -i lo -j ACCEPT
    run "$t" -A MAYA2C-IN -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    # SSH before anything that could drop: a firewall applied without it
    # loses a remote host.
    run "$t" -A MAYA2C-IN -p tcp --dport 22 -j ACCEPT
    for port in $(fw_ports); do
        run "$t" -A MAYA2C-IN -p tcp --dport "$port" -m conntrack --ctstate NEW -j ACCEPT
    done
    if [ "$t" = iptables ]; then
        run "$t" -A MAYA2C-IN -p icmp -j ACCEPT
    else
        run "$t" -A MAYA2C-IN -p ipv6-icmp -j ACCEPT
    fi
    run "$t" -A MAYA2C-IN -j DROP
    "$t" -C INPUT -j MAYA2C-IN 2>/dev/null || run "$t" -I INPUT 1 -j MAYA2C-IN
}

firewall() {
    log "firewall: ssh, p2p $P2P_BASE, chat $CHAT_PORT and the gateway open; everything else, node RPC included, dropped"
    # iptables-persistent asks two questions on install; answer them first.
    run sh -c "echo 'iptables-persistent iptables-persistent/autosave_v4 boolean false' | debconf-set-selections"
    run sh -c "echo 'iptables-persistent iptables-persistent/autosave_v6 boolean false' | debconf-set-selections"
    run env DEBIAN_FRONTEND=noninteractive "${APT[@]}" install -y -qq iptables iptables-persistent
    if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
        log "disabling ufw: its rules would sit alongside ours and confuse the next reader"
        run ufw --force disable
    fi
    fw_chain iptables
    fw_chain ip6tables
    run netfilter-persistent save
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
    handle_path /faucet/* {
        reverse_proxy $FAUCET_LOCAL
    }
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
    # Seed mode only: a joining node has no faucet key and funds nobody.
    if [ -f "$CONF/faucet.env" ] || [ "$DRY_RUN" = 1 ]; then
        run systemctl enable maya2c-faucet
        run systemctl restart maya2c-faucet
    fi
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
                if [ -f "$CONF/faucet.env" ]; then
                    local fdeadline=$((SECONDS + 60))
                    until curl -fsS --max-time 5 -o /dev/null "http://127.0.0.1:$FAUCET_PORT/health"; do
                        [ $SECONDS -lt $fdeadline ] || die "the faucet does not answer /health (journalctl -u maya2c-faucet)"
                        sleep 2
                    done
                    log "faucet healthy"
                fi
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
    # The relay listens on 0.0.0.0 and logs one line per interface; its
    # public address is this host's IP with the relay's peer id.
    local peer
    peer=$(journalctl -u maya-chat-relay --no-pager -o cat | grep -o '/p2p/[A-Za-z0-9]*' | tail -1)
    relay="/ip4/$ip/tcp/$CHAT_PORT$peer"
    cat <<EOF

[testnet] UP — $(jq -r .chain_id "$CONF/genesis.json")
  gateway      ${DOMAIN:+https://$DOMAIN}${DOMAIN:-http://$ip:8080}
  chat relay   $relay
  faucet       ${DOMAIN:+https://$DOMAIN/faucet/request}${DOMAIN:-http://$ip:$FAUCET_PORT/request}   (POST {"address":"<hex>"})
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
