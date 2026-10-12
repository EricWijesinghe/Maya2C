#!/usr/bin/env bash
# One command from a fresh Mac or Linux machine to a maya-testnet-1 node:
#
#   curl -fsSL https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.sh | bash
#
# Written for people who are helping, not for engineers: every step says
# what it does in plain words, every failure says what to do next, and the
# same file is installed as `maya2c-validator` so the follow-up commands
# (status, apply, register, logs, update, uninstall) need no paths.
#
# What it does, and refuses to skip:
#   1. downloads the newest testnet release for this CPU and checks it
#      against the release's SHA256SUMS;
#   2. downloads genesis.json and checks it against the hash pinned below,
#      so a tampered site cannot point you at another chain;
#   3. makes a validator key (kept on this machine only, owner-only
#      permissions) unless one exists; registering it is a separate,
#      reviewed step (`maya2c-validator apply`);
#   4. installs a background service (systemd on Linux, launchd on macOS)
#      that joins through the public WebSocket bootnode, loads a snapshot,
#      follows the chain and restarts itself.
#
# Options (environment): MAYA_HOME (where keys and chain data live),
# MAYA_NO_SERVICE=1 to install without a service, MAYA_RPC_PORT (default 8545).
#
# It never prints or sends a secret. Re-running it upgrades the binary and
# leaves the keys and the chain data alone.
#
# bash 3.2 compatible on purpose: that is the bash every Mac ships.
set -euo pipefail

REPO="EricWijesinghe/Maya2C"
SCRIPT_URL="https://raw.githubusercontent.com/$REPO/master/scripts/join-testnet.sh"
GENESIS_URL="https://maya2c.dev/testnet/genesis.json"
GENESIS_SHA256="dd9bb356c94b7699e6e8d2595681faee48be435b02dc927677cbf833e55edd5d"
BOOTNODE="/dns4/p2p.maya2c.dev/tcp/443/wss/p2p/12D3KooWJqH5AUQ1zPkMke18rcc8VHN2TWQ5XudGX3tP8zPLuyjB"
BOOTSTRAP="https://bootstrap.maya2c.dev/rpc"
PUBLIC_RPC="https://rpc.maya2c.dev/rpc"
APPLY_URL="https://github.com/$REPO/issues/new?template=validator-application.yml"
RPC_PORT="${MAYA_RPC_PORT:-8545}"  # loopback only: the node's RPC is unauthenticated
PRUNE_DEPTH=3600       # one epoch: deep enough to accept the newest snapshot
SYNCED_WITHIN=5        # blocks behind the network that still count as "in sync"
BIN_DIR=/usr/local/bin
HELPER="$BIN_DIR/maya2c-validator"
MAC_LABEL="dev.maya2c.node"
MAC_PLIST="/Library/LaunchDaemons/$MAC_LABEL.plist"

OS=$(uname -s)
case "$OS" in
  Linux)
    HOME_DIR="${MAYA_HOME:-/var/lib/maya2c}"
    LOG_FILE="" ;;                      # journald holds the log
  Darwin)
    HOME_DIR="${MAYA_HOME:-/Library/Application Support/Maya2C}"
    LOG_FILE="/Library/Logs/Maya2C/node.log" ;;  # shows up in Console.app
  *) printf 'maya2c: this script is for Mac and Linux. On Windows see https://maya2c.dev/guides/help-run/\n' >&2; exit 1 ;;
esac
KEY="$HOME_DIR/validator.key"
PUB="$HOME_DIR/validator.pub"
WALLET="$HOME_DIR/wallet.key"
DATA="$HOME_DIR/data"

ok()   { printf '  \033[1;32m✔\033[0m %s\n' "$*"; }
say()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die()  { printf '\n\033[1;31m✘ %s\033[0m\n' "$1" >&2; [ -n "${2:-}" ] && printf '  What to do: %s\n' "$2" >&2; exit 1; }

SUDO=""; [ "$(id -u)" -eq 0 ] || SUDO="sudo"
# The account the Mac service runs as: the person who ran the installer,
# never root. Linux uses a dedicated system user instead.
RUN_USER="${SUDO_USER:-$(id -un)}"

sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

need() { command -v "$1" >/dev/null || die "this computer is missing '$1'" "${2:-install $1, then run this again}"; }

release_target() {
  case "$OS-$(uname -m)" in
    Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
    Linux-aarch64|Linux-arm64) echo aarch64-unknown-linux-gnu ;;
    Darwin-*)
      # Under Rosetta, uname -m says x86_64 on an Apple-silicon Mac.
      if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
        echo aarch64-apple-darwin
      else
        echo x86_64-apple-darwin
      fi ;;
    *) die "there is no Maya2C build for this processor ($(uname -m)) yet" "ask in the Maya2C chat; this needs a new build" ;;
  esac
}

# The newest testnet release that has a build for this machine.
find_release() {
  local target=$1 tag
  tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=20" \
    | grep -o "\"browser_download_url\": *\"[^\"]*/node-[^\"/]*testnet[^\"/]*/maya2c-$target.tar.gz\"" \
    | head -1 | sed 's#.*/download/\([^/]*\)/.*#\1#') || true
  [ -n "$tag" ] || die "no testnet release has a build for $target yet" \
    "an Intel Mac build arrives with the next release; ask in the Maya2C chat"
  echo "$tag"
}

install_binaries() {
  local target tag base want have
  target=$(release_target)
  tag=$(find_release "$target")
  say "Step 1 of 4 — downloading Maya2C $tag"
  # Global, not local: the EXIT trap runs after this function has returned.
  work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
  base="https://github.com/$REPO/releases/download/$tag"
  curl -fsSL -o "$work/pkg.tar.gz" "$base/maya2c-$target.tar.gz" \
    || die "the download failed" "check the internet connection and run the same command again"
  curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS"
  want=$(grep " maya2c-$target.tar.gz\$" "$work/SHA256SUMS" | cut -d' ' -f1)
  have=$(sha256_of "$work/pkg.tar.gz")
  [ -n "$want" ] && [ "$want" = "$have" ] \
    || die "the download does not match its published checksum, so it was NOT installed" "run the command again; if it repeats, tell Eric"
  ok "download verified"
  tar xzf "$work/pkg.tar.gz" -C "$work"
  $SUDO mkdir -p "$BIN_DIR"   # a fresh Apple-silicon Mac has no /usr/local/bin
  $SUDO install -m 755 "$work/maya2c-$target/maya2c-node" "$work/maya2c-$target/l1-wallet" "$BIN_DIR/"
  # curl sets no quarantine flag, so Gatekeeper never sees these files; clear
  # it anyway in case they arrived some other way.
  [ "$OS" = Darwin ] && $SUDO xattr -d com.apple.quarantine "$BIN_DIR/maya2c-node" "$BIN_DIR/l1-wallet" 2>/dev/null || true
  ok "installed maya2c-node and l1-wallet in $BIN_DIR"
}

install_genesis() {
  local tmp
  say "Step 2 of 4 — getting the network's starting block (genesis)"
  $SUDO mkdir -p "$HOME_DIR"
  tmp=$(mktemp)
  curl -fsSL -o "$tmp" "$GENESIS_URL" || die "could not download genesis" "check the internet connection and run the command again"
  [ "$(sha256_of "$tmp")" = "$GENESIS_SHA256" ] \
    || die "genesis does not match the published hash, so this would be the wrong network" "tell Eric; do not continue"
  $SUDO install -m 644 "$tmp" "$HOME_DIR/genesis.json"; rm -f "$tmp"
  ok "genesis verified"
}

make_key() {
  say "Step 3 of 4 — your validator key"
  if $SUDO test -f "$KEY"; then ok "kept your existing key"; return; fi
  local pub
  pub=$($SUDO "$BIN_DIR/maya2c-node" --generate-validator-key "$KEY")
  $SUDO chmod 600 "$KEY"
  printf '%s\n' "$pub" | $SUDO tee "$PUB" >/dev/null
  ok "made a new key. It never leaves this computer."
}

node_args() {
  printf '%s\n' --genesis "$HOME_DIR/genesis.json" --data-dir "$DATA" \
    --validator-key "$KEY" --rpc-addr "127.0.0.1:$RPC_PORT" --bootnode "$BOOTNODE" \
    --prune-depth "$PRUNE_DEPTH" --bootstrap-from "$BOOTSTRAP" --catch-up-from "$BOOTSTRAP"
  # --bootstrap-from loads a snapshot into an empty data dir only; the node
  # ignores it once a chain is there, so the service can always pass it.
}

install_linux_service() {
  local args
  args=$(node_args | tr '\n' ' ')
  id maya2c >/dev/null 2>&1 || $SUDO useradd --system --home "$HOME_DIR" --shell /usr/sbin/nologin maya2c
  $SUDO chown -R maya2c: "$HOME_DIR"; $SUDO chmod 700 "$HOME_DIR"
  $SUDO tee /etc/systemd/system/maya2c-node.service >/dev/null <<UNIT
[Unit]
Description=Maya2C node (maya-testnet-1)
After=network-online.target
Wants=network-online.target

[Service]
User=maya2c
ExecStart=$BIN_DIR/maya2c-node $args
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
  $SUDO systemctl enable maya2c-node >/dev/null 2>&1
  $SUDO systemctl restart maya2c-node
}

install_mac_service() {
  local arg items=""
  while IFS= read -r arg; do items="$items    <string>$arg</string>
"; done < <(node_args)
  $SUDO mkdir -p "$(dirname "$LOG_FILE")"; $SUDO touch "$LOG_FILE"
  $SUDO chown -R "$RUN_USER" "$HOME_DIR" "$(dirname "$LOG_FILE")"; $SUDO chmod 700 "$HOME_DIR"
  # caffeinate -s keeps the Mac awake while it is plugged in, so the node does
  # not drop out when the screen locks. On battery the Mac may still sleep.
  $SUDO tee "$MAC_PLIST" >/dev/null <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$MAC_LABEL</string>
  <key>UserName</key><string>$RUN_USER</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/caffeinate</string>
    <string>-s</string>
    <string>$BIN_DIR/maya2c-node</string>
$items  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>5</integer>
  <key>StandardOutPath</key><string>$LOG_FILE</string>
  <key>StandardErrorPath</key><string>$LOG_FILE</string>
</dict>
</plist>
PLIST
  $SUDO chmod 644 "$MAC_PLIST"
  $SUDO launchctl bootout "system/$MAC_LABEL" 2>/dev/null || true
  $SUDO launchctl bootstrap system "$MAC_PLIST" || die "macOS refused to start the service" "restart the Mac and run the command again"
}

install_helper() {
  local tmp self="${BASH_SOURCE[0]:-}"
  # Run from a file (a checkout, CI): install that file. Under `curl | bash`
  # there is no file, so fetch the published copy.
  if [ -n "$self" ] && [ -f "$self" ]; then
    $SUDO install -m 755 "$self" "$HELPER"; return
  fi
  tmp=$(mktemp)
  if curl -fsSL -o "$tmp" "$SCRIPT_URL"; then
    $SUDO install -m 755 "$tmp" "$HELPER"
  fi
  rm -f "$tmp"
}

cmd_install() {
  need curl; need tar
  printf '\n\033[1mMaya2C testnet node — installer\033[0m\n'
  note "This installs a background program that helps run the Maya2C test network."
  note "Testnet coins have no value. You can remove everything with: maya2c-validator uninstall"
  [ -n "$SUDO" ] && note "Your computer password may be asked for. Nothing shows while you type it — that is normal."
  echo
  install_binaries
  install_genesis
  make_key
  say "Step 4 of 4 — starting the node in the background"
  if [ "$OS" = Darwin ]; then install_mac_service
  elif [ "${MAYA_NO_SERVICE:-}" != 1 ] && command -v systemctl >/dev/null; then install_linux_service
  else
    note "No systemd here. Start the node yourself with:"
    note "$BIN_DIR/maya2c-node $(node_args | tr '\n' ' ')"
  fi
  install_helper
  ok "running. It starts by itself whenever the computer starts."
  printf '\n\033[1;32mDone. Thank you for helping!\033[0m\n'
  note "Check on it any time:        maya2c-validator status"
  note "Become a validator (next):   maya2c-validator apply"
  note "The first start downloads the chain and can take up to an hour; 'status' shows the progress."
}

rpc() { # rpc <url> <method>: prints the raw JSON reply, or nothing
  curl -fsS -m 5 -X POST "$1" -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[]}" 2>/dev/null || true
}

height_at() { rpc "$1" get_tip_height | grep -o '"result":[0-9]*' | cut -d: -f2 || true; }

cmd_status() {
  local mine net
  printf '\n\033[1mMaya2C node status\033[0m\n'
  if pgrep -x maya2c-node >/dev/null; then ok "the node is running"; else
    die "the node is not running" "run: maya2c-validator restart   (then check again in a minute)"; fi
  mine=$(height_at "http://127.0.0.1:$RPC_PORT")
  net=$(height_at "$BOOTSTRAP")
  if [ -z "$mine" ]; then note "⏳ downloading the chain — the first start can take up to an hour"
  elif [ -n "$net" ] && [ $((net - mine)) -le "$SYNCED_WITHIN" ]; then ok "in sync: block $mine (network: $net)"
  else note "⏳ catching up: block $mine of ${net:-?}"
       note "  The first start can take up to an hour. Nothing to do but wait."; fi
  # A committee member has a numeric "validator" index; an observer has null.
  if rpc "http://127.0.0.1:$RPC_PORT" get_bft_status | grep -q '"validator":[0-9]'; then
    ok "validating: your node is in the committee and voting"
  else
    note "• following the chain (not in the validator committee yet — see: maya2c-validator apply)"
  fi
  note "your files: $HOME_DIR"
  if [ "$OS" = Darwin ]; then note "log: $LOG_FILE (or: maya2c-validator logs)"; else note "log: maya2c-validator logs"; fi
}

wallet() { $SUDO env L1_WALLET_KEYSTORE="$WALLET" "$BIN_DIR/l1-wallet" --rpc-url "$PUBLIC_RPC" "$@"; }

cmd_apply() {
  local address
  $SUDO test -f "$PUB" || die "no validator key yet" "run the installer first"
  if ! $SUDO test -f "$WALLET"; then
    say "Making your wallet. Choose a password and write it down — it cannot be recovered."
    wallet generate >/dev/null
    $SUDO chmod 600 "$WALLET"
  fi
  address=$(wallet address | tail -1 | grep -o '[0-9a-f]\{64\}' || true)
  [ -n "$address" ] || die "could not read the wallet address" "run: maya2c-validator apply   again"
  { printf 'Maya2C validator application\n\nWallet address:\n%s\n\nValidator public key:\n' "$address"
    $SUDO cat "$PUB"; } > "$HOME/maya2c-application.txt"
  printf '\n\033[1mYour application is ready:\033[0m %s\n\n' "$HOME/maya2c-application.txt"
  note "It holds only PUBLIC information. Send it to Eric any way you like,"
  note "or open the application form and paste the two values in:"
  note "  $APPLY_URL"
  note "When Eric says you are approved and funded, run:"
  note "  maya2c-validator register <the amount Eric gives you>"
  [ "$OS" = Darwin ] && open -R "$HOME/maya2c-application.txt" 2>/dev/null || true
}

cmd_register() {
  local bond=${1:-}
  case "$bond" in ''|*[!0-9]*) die "give the bond amount as a number" "e.g.  maya2c-validator register 1000000";; esac
  $SUDO test -f "$WALLET" || die "no wallet yet" "run: maya2c-validator apply"
  say "Registering your validator (your wallet password is needed)"
  wallet register-validator --validator-key "$KEY" --bond "$bond"
  ok "registered. You join the committee at the next epoch (within about an hour)."
  note "Check with: maya2c-validator status"
}

cmd_logs() {
  if [ "$OS" = Darwin ]; then tail -n 50 -f "$LOG_FILE"; else journalctl -u maya2c-node -n 50 -f; fi
}

cmd_service() { # start | stop | restart
  if [ "$OS" = Darwin ]; then
    case "$1" in
      stop) $SUDO launchctl bootout "system/$MAC_LABEL" 2>/dev/null || true ;;
      *) $SUDO launchctl bootout "system/$MAC_LABEL" 2>/dev/null || true
         $SUDO launchctl bootstrap system "$MAC_PLIST" ;;
    esac
  else $SUDO systemctl "$1" maya2c-node; fi
  ok "$1: done"
}

cmd_uninstall() {
  say "Removing the Maya2C node"
  if [ "$OS" = Darwin ]; then
    $SUDO launchctl bootout "system/$MAC_LABEL" 2>/dev/null || true
    $SUDO rm -f "$MAC_PLIST"; $SUDO rm -rf "$(dirname "$LOG_FILE")"
  elif command -v systemctl >/dev/null; then
    $SUDO systemctl disable --now maya2c-node 2>/dev/null || true
    $SUDO rm -f /etc/systemd/system/maya2c-node.service; $SUDO systemctl daemon-reload
  fi
  $SUDO rm -rf "$DATA"
  $SUDO rm -f "$BIN_DIR/maya2c-node" "$BIN_DIR/l1-wallet" "$HELPER"
  ok "removed the program and the chain data"
  # Keys are never deleted silently: a lost validator key costs its seat.
  note "Your keys are kept in: $HOME_DIR"
  note "Delete that folder too only if you will never validate again."
}

usage() {
  cat <<USAGE
maya2c-validator — look after your Maya2C testnet node

  status            is it running, in sync, validating?
  apply             make your wallet and the application to send Eric
  register <bond>   after approval: register as a validator
  logs              watch what the node is doing (Ctrl+C to stop watching)
  restart | stop | start
  update            install the newest version (keeps keys and data)
  uninstall         remove it (keeps your keys)
USAGE
}

case "${1:-install}" in
  install)        cmd_install ;;
  # The newest installer, not this copy of it: an update may fix the installer.
  update)         curl -fsSL "$SCRIPT_URL" | bash ;;
  status)         cmd_status ;;
  apply)          cmd_apply ;;
  register)       cmd_register "${2:-}" ;;
  logs)           cmd_logs ;;
  start|stop|restart) cmd_service "$1" ;;
  uninstall)      cmd_uninstall ;;
  help|-h|--help) usage ;;
  *) usage; exit 1 ;;
esac
