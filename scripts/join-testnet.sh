#!/usr/bin/env bash
# One command from a fresh Linux machine to a maya-testnet-1 node following
# the chain:
#
#   curl -fsSL https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.sh | bash
#
# What it does, and refuses to skip:
#   1. downloads the newest testnet release for this CPU and checks it
#      against the release's SHA256SUMS;
#   2. downloads genesis.json and checks it against the hash pinned below,
#      so a tampered site cannot point you at another chain;
#   3. makes a validator key (kept on this machine only, mode 600) unless
#      one exists — registering it is a separate, reviewed step;
#   4. installs a systemd service that joins through the public WebSocket
#      bootnode, loads a snapshot and follows the chain.
#
# It never asks for, prints or sends a secret. Re-running it upgrades the
# binary and leaves the key and the chain data alone.
#
# Options (environment): MAYA_HOME (default /var/lib/maya2c),
# MAYA_NO_SERVICE=1 to install without systemd.
set -euo pipefail

REPO="EricWijesinghe/Maya2C"
GENESIS_URL="https://maya2c.dev/testnet/genesis.json"
GENESIS_SHA256="dd9bb356c94b7699e6e8d2595681faee48be435b02dc927677cbf833e55edd5d"
BOOTNODE="/dns4/p2p.maya2c.dev/tcp/443/wss/p2p/12D3KooWK2bykmqyzdK4TMSTBQ5visSjkFoK8wj88aMn3ms4ByiG"
BOOTSTRAP="https://bootstrap.maya2c.dev/rpc"
HOME_DIR="${MAYA_HOME:-/var/lib/maya2c}"

say() { printf '\033[1;34mmaya2c\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mmaya2c\033[0m %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "needs $1"; }
need curl; need tar; need sha256sum

case "$(uname -m)" in
  x86_64) target=x86_64-unknown-linux-gnu ;;
  aarch64|arm64) target=aarch64-unknown-linux-gnu ;;
  *) die "no release for $(uname -m)" ;;
esac
SUDO=""; [ "$(id -u)" -eq 0 ] || SUDO="sudo"

tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=20" \
  | grep -o '"tag_name": *"node-[^"]*testnet[^"]*"' | head -1 | cut -d'"' -f4)
[ -n "$tag" ] || die "no testnet release found"
say "release $tag for $target"

work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
base="https://github.com/$REPO/releases/download/$tag"
curl -fsSL -o "$work/pkg.tar.gz" "$base/maya2c-$target.tar.gz"
curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS"
want=$(grep " maya2c-$target.tar.gz\$" "$work/SHA256SUMS" | cut -d' ' -f1)
have=$(sha256sum "$work/pkg.tar.gz" | cut -d' ' -f1)
[ -n "$want" ] && [ "$want" = "$have" ] || die "release checksum mismatch: refusing to install"
say "checksum ok"
tar xzf "$work/pkg.tar.gz" -C "$work"
$SUDO install -m 755 "$work/maya2c-$target/maya2c-node" "$work/maya2c-$target/l1-wallet" /usr/local/bin/

$SUDO mkdir -p "$HOME_DIR"
curl -fsSL -o "$work/genesis.json" "$GENESIS_URL"
[ "$(sha256sum "$work/genesis.json" | cut -d' ' -f1)" = "$GENESIS_SHA256" ] \
  || die "genesis hash mismatch: refusing to join"
$SUDO install -m 644 "$work/genesis.json" "$HOME_DIR/genesis.json"
say "genesis ok"

if [ ! -f "$HOME_DIR/validator.key" ]; then
  umask 077
  pub=$($SUDO /usr/local/bin/maya2c-node --generate-validator-key "$HOME_DIR/validator.key")
  $SUDO chmod 600 "$HOME_DIR/validator.key"
  printf '%s\n' "$pub" | $SUDO tee "$HOME_DIR/validator.pub" >/dev/null
  say "new validator key; its PUBLIC key is in $HOME_DIR/validator.pub"
fi

args="--genesis $HOME_DIR/genesis.json --data-dir $HOME_DIR/data --rpc-addr 127.0.0.1:8545 \
--bootnode $BOOTNODE --prune-depth 3600 --catch-up-from $BOOTSTRAP --bootstrap-from $BOOTSTRAP"
# --bootstrap-from loads a snapshot into an empty data dir only; the node
# ignores it once a chain is there, so the service can always pass it.

if [ "${MAYA_NO_SERVICE:-}" = 1 ] || ! command -v systemctl >/dev/null; then
  say "run it with: maya2c-node $args"
  exit 0
fi
id maya2c >/dev/null 2>&1 || $SUDO useradd --system --home "$HOME_DIR" --shell /usr/sbin/nologin maya2c
$SUDO chown -R maya2c: "$HOME_DIR"
$SUDO tee /etc/systemd/system/maya2c-node.service >/dev/null <<UNIT
[Unit]
Description=Maya2C node (maya-testnet-1)
After=network-online.target
Wants=network-online.target

[Service]
User=maya2c
ExecStart=/usr/local/bin/maya2c-node $args
Restart=always
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$HOME_DIR

[Install]
WantedBy=multi-user.target
UNIT
$SUDO systemctl daemon-reload
$SUDO systemctl enable --now maya2c-node
say "running. Follow it with: journalctl -u maya2c-node -f"
say "to validate: apply with the public key in $HOME_DIR/validator.pub (see https://maya2c.dev/guides/validators/)"
