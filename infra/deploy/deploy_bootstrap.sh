#!/usr/bin/env bash
#
# Bare-metal bootstrap for a Maya2C node.
#
# =============================================================================
# NOT VERIFIED ON A LIVE SYSTEM
# =============================================================================
#
# Written on a Windows host with no Linux, no UFW, no certbot, and no
# shellcheck. The logic has been read carefully; it has not been run. Review it
# and dry-run it on a disposable host before pointing it at anything you care
# about.
#
# =============================================================================
# What this does, and one thing it deliberately does not
# =============================================================================
#
#   1. Installs the systemd units and a config.toml.
#   2. Configures UFW: P2P open to the world, RPC bound to loopback only.
#   3. Fetches genesis.json and VERIFIES its state root against a commitment
#      you supply, refusing to proceed on a mismatch.
#   4. Optionally provisions TLS — for the API GATEWAY, not the node's RPC.
#
# It does NOT terminate TLS on the node's JSON-RPC, and that is the point of
# the exercise rather than an omission.
#
# The node's RPC has no authentication and serves `get_mining_candidate` and
# `submit_block`. `maya-api-gateway` exists to be the public surface, with a
# default-deny allowlist that excludes both. Putting a certificate on the
# node's RPC and opening 8545 would publish the miner interface to the internet
# with a padlock next to it.
#
# So: RPC is firewalled to loopback here, and TLS goes on the gateway.
#
# =============================================================================
# There are no "genesis DAG checkpoints" to seed
# =============================================================================
#
# `crypto/dag` is the Ethash-style memory-hard proof-of-work DATASET, derived
# deterministically on each node from its epoch seed. It is generated, not
# distributed, and there is nothing to check point.
#
# What genesis actually has is `genesis.json`, a state root, and a genesis
# block id — the three values on the ceremony's COMMITMENT.txt. Verifying those
# is what step 3 does, and it is the check that actually tells an operator
# whether they joined the network they meant to.

set -euo pipefail

# --- configuration -----------------------------------------------------------

GENESIS_URL="${GENESIS_URL:-}"
EXPECTED_STATE_ROOT="${EXPECTED_STATE_ROOT:-}"
GATEWAY_DOMAIN="${GATEWAY_DOMAIN:-}"
CERTBOT_EMAIL="${CERTBOT_EMAIL:-}"
P2P_PORT="${P2P_PORT:-30333}"
BIN_DIR="${BIN_DIR:-/usr/local/bin}"
CONF_DIR="${CONF_DIR:-/etc/maya2c}"
DRY_RUN="${DRY_RUN:-0}"

# Chain ids the node itself refuses to start on. Checked here too so an
# operator learns before provisioning a host rather than after.
VALUE_BEARING_CHAINS=("mainnet" "maya-mainnet")

log()  { printf '[bootstrap] %s\n' "$*"; }
die()  { printf '[bootstrap] error: %s\n' "$*" >&2; exit 1; }
run()  {
    if [ "$DRY_RUN" = "1" ]; then
        printf '[dry-run] %s\n' "$*"
    else
        "$@"
    fi
}

usage() {
    cat <<'USAGE'
deploy_bootstrap.sh — provision a Maya2C node on bare metal

Environment:
  GENESIS_URL          where to fetch genesis.json (required)
  EXPECTED_STATE_ROOT  hex state root from the ceremony COMMITMENT.txt (required)
  GATEWAY_DOMAIN       domain for the API gateway's TLS certificate (optional)
  CERTBOT_EMAIL        contact address for Let's Encrypt (required with the above)
  P2P_PORT             libp2p listen port (default 30333)
  DRY_RUN=1            print what would be done and change nothing

Example:
  GENESIS_URL=https://example.invalid/genesis.json \
  EXPECTED_STATE_ROOT=45e2811b...f15b \
  DRY_RUN=1 ./deploy_bootstrap.sh
USAGE
}

# --- preflight ---------------------------------------------------------------

[ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ] && { usage; exit 0; }

[ -n "$GENESIS_URL" ] || die "GENESIS_URL is required (see --help)"
[ -n "$EXPECTED_STATE_ROOT" ] || die "EXPECTED_STATE_ROOT is required; it is the
one value that tells you whether you joined the network you meant to"

if [ "$(id -u)" -ne 0 ] && [ "$DRY_RUN" != "1" ]; then
    die "run as root, or set DRY_RUN=1"
fi

for tool in curl jq systemctl; do
    command -v "$tool" >/dev/null 2>&1 || die "missing required tool: $tool"
done

# --- 1. genesis, fetched and VERIFIED ---------------------------------------
#
# Verified before anything is installed. A host provisioned against the wrong
# genesis is a host that will never sync, and the symptom — zero peers — looks
# like a firewall problem for as long as it takes somebody to check.

log "fetching genesis from $GENESIS_URL"
GENESIS_TMP="$(mktemp)"
trap 'rm -f "$GENESIS_TMP"' EXIT
run curl -fsSL --max-time 60 -o "$GENESIS_TMP" "$GENESIS_URL"

if [ "$DRY_RUN" != "1" ]; then
    CHAIN_ID="$(jq -r '.chain_id' "$GENESIS_TMP")"
    [ "$CHAIN_ID" != "null" ] || die "genesis.json has no chain_id"
    log "genesis chain_id: $CHAIN_ID"

    for blocked in "${VALUE_BEARING_CHAINS[@]}"; do
        if [ "$CHAIN_ID" = "$blocked" ]; then
            die "chain id '$CHAIN_ID' is value-bearing and the node will refuse to
start on it. The shielded pool's circuit has not been independently audited.
See docs/mainnet-readiness.md section 1."
        fi
    done

    # The state root is computed by the node from the genesis file, so this
    # compares what the file implies against what the ceremony recorded. A
    # mismatch means the file was edited, truncated, or is from another network.
    ACTUAL_ROOT="$("$BIN_DIR/genesis" --verify "$GENESIS_TMP" 2>/dev/null \
        | awk '/state root/ {print $NF}' || true)"

    if [ -z "$ACTUAL_ROOT" ]; then
        log "warning: could not compute the state root locally ($BIN_DIR/genesis
missing or does not support --verify). Verify it by hand against COMMITMENT.txt
before starting the node."
    elif [ "$ACTUAL_ROOT" != "$EXPECTED_STATE_ROOT" ]; then
        die "state root mismatch.
  expected  $EXPECTED_STATE_ROOT
  computed  $ACTUAL_ROOT
This genesis file is not the one the commitment describes. Do not proceed."
    else
        log "state root verified: $ACTUAL_ROOT"
    fi
fi

# --- 2. install --------------------------------------------------------------

log "installing configuration into $CONF_DIR"
run mkdir -p "$CONF_DIR"
run install -m 0644 "$GENESIS_TMP" "$CONF_DIR/genesis.json"

if [ ! -f "$CONF_DIR/config.toml" ]; then
    log "writing a default config.toml"
    # Generated by the binary from its own defaults, so it cannot go stale.
    run sh -c "'$BIN_DIR/node' --print-config-template > '$CONF_DIR/config.toml'"
else
    log "config.toml exists; leaving it alone"
fi

log "installing systemd units"
run install -m 0644 infra/deploy/systemd/maya2c-node.service /etc/systemd/system/
run install -m 0644 infra/deploy/systemd/maya2c-miner.service /etc/systemd/system/
run systemctl daemon-reload

# --- 3. firewall -------------------------------------------------------------
#
# Default deny inbound. P2P is the only port the world needs.

if command -v ufw >/dev/null 2>&1; then
    log "configuring UFW"
    run ufw --force default deny incoming
    run ufw --force default allow outgoing

    # SSH first. A default-deny rule applied before an SSH allow is how a
    # remote host is lost.
    run ufw allow 22/tcp comment 'ssh'

    run ufw allow "$P2P_PORT/tcp" comment 'maya2c p2p'

    # RPC is NOT opened. It binds 127.0.0.1 by default in config.toml and this
    # is the second line of defence. If you find yourself adding
    # `ufw allow 8545`, read the header of this file again.
    log "RPC port is deliberately NOT opened; maya-api-gateway is the public surface"

    run ufw --force enable
    run ufw status verbose
else
    log "warning: ufw not installed; the firewall was NOT configured"
fi

# --- 4. TLS, on the gateway ---------------------------------------------------

if [ -n "$GATEWAY_DOMAIN" ]; then
    [ -n "$CERTBOT_EMAIL" ] || die "CERTBOT_EMAIL is required with GATEWAY_DOMAIN"
    command -v certbot >/dev/null 2>&1 || die "certbot is not installed"

    log "provisioning TLS for the API GATEWAY at $GATEWAY_DOMAIN"
    log "(not for the node's RPC — see the header of this file)"

    run ufw allow 80/tcp comment 'acme http-01'
    run ufw allow 443/tcp comment 'maya2c api gateway'

    run certbot certonly --standalone \
        --non-interactive --agree-tos \
        --email "$CERTBOT_EMAIL" \
        -d "$GATEWAY_DOMAIN"

    log "certificate at /etc/letsencrypt/live/$GATEWAY_DOMAIN/"
    log "point maya-api-gateway at it; do not point the node's RPC at it"
else
    log "no GATEWAY_DOMAIN set; skipping TLS"
fi

# --- done --------------------------------------------------------------------

cat <<'NEXT'

[bootstrap] provisioned. Not started — review first:

  systemctl cat maya2c-node.service
  systemd-analyze verify maya2c-node.service
  systemd-analyze security maya2c-node.service

Then:

  systemctl enable --now maya2c-node.service
  infra/deploy/health/check_peers.sh
  infra/deploy/health/check_propagation.sh

The units are deliberately not enabled by this script. A bootstrap that
starts a node is a bootstrap that has joined a network before anyone read
what it was configured with.
NEXT
