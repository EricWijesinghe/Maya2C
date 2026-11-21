#!/usr/bin/env bash
# Static guards for the installer, deploy and infra hardening. Each check
# names one setting and fails if that setting is reverted. No cluster, host
# or network is touched: this only reads tracked files.
#
# Usage: bash scripts/check-infra-hardening.sh   (from anywhere in the repo)
set -u
cd "$(dirname "$0")/.." || exit 1

fails=0
# ok_if DESCRIPTION COMMAND... : runs COMMAND, records pass or fail.
ok_if() {
    local desc="$1"; shift
    if "$@"; then printf 'ok    %s\n' "$desc"
    else printf 'FAIL  %s\n' "$desc"; fails=$((fails + 1)); fi
}
# Fixed-string presence and absence in one file.
has() { grep -qF -- "$2" "$1"; }
lacks() { ! grep -qF -- "$2" "$1"; }
# First line number of a fixed string, or nothing.
line_of() { grep -nF -- "$2" "$1" | head -1 | cut -d: -f1; }
# Line number of the line that holds just the given token (a call site).
call_line() { grep -nE "^[[:space:]]+$2[[:space:]]*\$" "$1" | head -1 | cut -d: -f1; }
before() { [ -n "$1" ] && [ -n "$2" ] && [ "$1" -lt "$2" ]; }

INSTALL=infra/testnet-vm/install.sh
COMPOSE=infra/docker/docker-compose.yml
DOCKERFILE=infra/docker/Dockerfile
MONITOR=infra/monitoring/docker-compose.monitoring.yml
HYBRID=infra/hybrid-testnet/start-testnet.ps1
VALIDATOR=infra/testnet-vm/join-validator.ps1
DEPLOY=scripts/deploy-production.sh

# Plain HTTP on a public interface (testnet VM without DOMAIN).
ok_if "gateway stays on loopback when DOMAIN is unset" has "$INSTALL" 'local listen="127.0.0.1:8080"'
ok_if "no public 0.0.0.0:8080 gateway listen" lacks "$INSTALL" 'listen="0.0.0.0:8080"'
ok_if "faucet stays on loopback when DOMAIN is unset" has "$INSTALL" 'local faucet_listen="127.0.0.1:$FAUCET_PORT" trust=false'
ok_if "firewall opens no plain-HTTP port without DOMAIN" lacks "$INSTALL" "printf '8080\\n%s\\n'"
ok_if "install summary no longer advertises public plain HTTP" lacks "$INSTALL" 'http://$ip:8080'

# Node RPC published on every host interface (miner methods included).
ok_if "compose seed RPC published on 127.0.0.1" has "$COMPOSE" '"127.0.0.1:8545:8545"'
ok_if "compose peer-1 RPC published on 127.0.0.1" has "$COMPOSE" '"127.0.0.1:8546:8545"'
ok_if "compose peer-2 RPC published on 127.0.0.1" has "$COMPOSE" '"127.0.0.1:8547:8545"'
ok_if "no compose RPC port bound to all interfaces" lacks "$COMPOSE" '"8545:8545"'
ok_if "Dockerfile no longer calls 8545 reachable from outside" lacks "$DOCKERFILE" 'Only 30333 and 8545 are meant to be'

# Grafana: loopback bind and no literal admin password in the tracked file.
ok_if "Grafana published on 127.0.0.1" has "$MONITOR" '"127.0.0.1:35311:3000"'
ok_if "Grafana admin password not a literal" lacks "$MONITOR" 'GF_SECURITY_ADMIN_PASSWORD=lab'
ok_if "Grafana admin password read from the environment, required" has "$MONITOR" 'GF_SECURITY_ADMIN_PASSWORD=${GRAFANA_ADMIN_PASSWORD:?'

# SSH tunnel to the live VM: a changed host key must fail closed.
ok_if "SSH tunnel does not trust an unseen host key on its own" lacks "$HYBRID" 'StrictHostKeyChecking=accept-new'
ok_if "SSH tunnel requires a pinned host key" has "$HYBRID" 'StrictHostKeyChecking=yes'

# Windows validator: the data directory is locked before the key and wallet
# are written into it, so they never sit readable by other local users.
lock_call=$(call_line "$VALIDATOR" 'Lock-DataDir')
apply_lock=$(grep -nE '^[[:space:]]+Lock-DataDir[[:space:]]*$' "$VALIDATOR" | sed -n 2p | cut -d: -f1)
gen_line=$(line_of "$VALIDATOR" '--generate-validator-key')
wallet_line=$(line_of "$VALIDATOR" 'Invoke-Wallet generate')
ok_if "data directory locked before the validator key is generated" before "$lock_call" "$gen_line"
ok_if "data directory locked before the wallet is generated (install path)" before "$lock_call" "$wallet_line"
ok_if "data directory locked before the wallet is generated (apply path)" before "$apply_lock" "$wallet_line"

# Deploy: the applied plan is the one that was reviewed, not a fresh plan.
ok_if "deploy applies no fresh plan with -auto-approve" lacks "$DEPLOY" '-auto-approve'
ok_if "deploy writes a saved plan" has "$DEPLOY" 'terraform plan -input=false -out=tfplan'
ok_if "deploy applies the saved plan file" has "$DEPLOY" 'terraform apply -input=false tfplan'

if [ "$fails" -gt 0 ]; then
    printf '%s check(s) failed\n' "$fails"
    exit 1
fi
printf 'all infra hardening checks passed\n'
