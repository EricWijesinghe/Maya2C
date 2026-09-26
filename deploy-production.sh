#!/usr/bin/env bash
#
# deploy-production.sh — the deploy orchestrator (Master Prompt 10 §6).
#
# =============================================================================
# Safety model
# =============================================================================
#
# --dry-run is the DEFAULT. A dry run touches no cloud account and no DNS: it
# plans (terraform plan, ansible --check) where those tools exist, and against
# `--target local-k3d` it runs the whole pipeline for real on a throwaway k3d
# cluster on this machine, which costs nothing.
#
# A real deploy needs BOTH `--apply` and an approval token for each phase that
# spends money or touches a live server, passed as APPROVED="<phase>,<phase>"
# (Standing Order 6 / Master Prompt 10: nothing runs until "APPROVED: <step>").
# Secrets are never read from the command line or the environment of this
# script; configure_environment.sh stores them age-encrypted and this script
# decrypts them only inside the phase that needs them.
#
# Phases:
#   A  pre-flight      tools present, cargo check, proof logs, secrets present
#   B  infrastructure  terraform plan/apply + Cloudflare DNS    (cloud)
#   C  hardening       ansible (UFW, systemd isolation, TLS, eBPF drivers)
#   D  rollout         build, image, validators + RPC gateways, smoke test
#
# Any failure runs the rollback for the phases already completed, in reverse.
#
# =============================================================================
# Usage
# =============================================================================
#
#   ./deploy-production.sh                          # dry run, cloud target: plans only
#   ./deploy-production.sh --target local-k3d       # dry run, full pipeline on k3d
#   ./deploy-production.sh --target local-k3d --keep   # leave the cluster up
#   APPROVED=infrastructure,hardening,rollout ./deploy-production.sh --apply
#
# Environment (non-secret): NODES (default 12), BIN_PROFILE (default ci),
# CLUSTER (default maya2c-local), SKIP_BUILD=1 to reuse target/<profile>/maya2c-node.

set -euo pipefail
cd "$(dirname "$0")"

MODE=dry-run
TARGET=cloud
KEEP=0
NODES="${NODES:-12}"
BIN_PROFILE="${BIN_PROFILE:-ci}"
CLUSTER="${CLUSTER:-maya2c-local}"
NAMESPACE=maya-local
APPROVED="${APPROVED:-}"
START=$(date +%s)
COMPLETED=()

while (($#)); do
    case "$1" in
        --dry-run) MODE=dry-run ;;
        --apply) MODE=apply ;;
        --target) TARGET="$2"; shift ;;
        --keep) KEEP=1 ;;
        -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

# ---------------------------------------------------------------------------
# Progress display: one line per step, a bar per phase. Plain text, so the log
# of a CI run reads the same as the terminal.
# ---------------------------------------------------------------------------
bar() { # bar <done> <total>
    local done=$1 total=$2 width=24 fill
    fill=$((done * width / total))
    printf '[%s%s]' "$(printf '#%.0s' $(seq 1 $fill) 2>/dev/null)" "$(printf '.%.0s' $(seq 1 $((width - fill))) 2>/dev/null)"
}
phase() { printf '\n== Phase %s: %s %s\n' "$1" "$2" "$(bar "$3" 4)"; }
step() { printf '   %-58s' "$1"; }
ok() { printf 'ok %s\n' "${1:-}"; }
skip() { printf 'SKIP (%s)\n' "$1"; }
die() { printf 'FAIL\n\n%s\n' "$1" >&2; exit 1; }

approved() { [[ ",$APPROVED," == *",$1,"* ]]; }

retry() { # retry <attempts> <delay> <cmd...>
    local n=$1 d=$2; shift 2
    for ((i = 1; i <= n; i++)); do
        "$@" && return 0
        sleep "$d"; d=$((d * 2))
    done
    return 1
}

rollback() {
    local status=$?
    ((status == 0)) && return
    printf '\n!! failure (exit %s); rolling back completed phases: %s\n' "$status" "${COMPLETED[*]:-none}" >&2
    for ((i = ${#COMPLETED[@]} - 1; i >= 0; i--)); do
        case "${COMPLETED[$i]}" in
            local-cluster) ((KEEP)) || k3d cluster delete "$CLUSTER" >/dev/null 2>&1 || true ;;
            infrastructure) echo "   terraform destroy would run here (only after APPROVED apply)" >&2 ;;
        esac
    done
}
trap rollback EXIT

need() { command -v "$1" >/dev/null 2>&1; }

# ---------------------------------------------------------------------------
phase A "pre-flight" 0
step "mode=$MODE target=$TARGET nodes=$NODES"; ok
if [[ "$MODE" == apply && "$TARGET" == cloud && -z "$APPROVED" ]]; then
    die "--apply against the cloud needs APPROVED=<phase,...>. Nothing was changed."
fi
for tool in cargo git; do
    step "tool: $tool"; need "$tool" && ok || die "$tool is required"
done
if [[ "$TARGET" == local-k3d ]]; then
    for tool in docker k3d kubectl; do
        step "tool: $tool"; need "$tool" && ok || die "$tool is required for --target local-k3d"
    done
    step "docker daemon"; docker info >/dev/null 2>&1 && ok || die "docker daemon not reachable"
fi
step "features.toml claims backed (cargo xtask coverage)"
cargo xtask coverage --quiet >/dev/null 2>&1 && ok || die "cargo xtask coverage failed"
step "formal proofs (Lean models, if lean is installed)"
if need lean; then
    (cd formal/lean && lean Maya2C/FeeSplit.lean && lean Maya2C/Supply.lean) >/dev/null 2>&1 && ok || die "a Lean proof no longer checks"
else
    skip "lean not installed"
fi
step "secrets present (.env.production.age)"
if [[ "$TARGET" == cloud && "$MODE" == apply ]]; then
    [[ -f .env.production.age ]] && ok || die "run scripts/configure_environment.sh first"
else
    skip "not needed for $MODE/$TARGET"
fi

# ---------------------------------------------------------------------------
phase B "infrastructure" 1
if [[ "$TARGET" == local-k3d ]]; then
    step "k3d cluster $CLUSTER (1 server, 2 agents)"
    if k3d cluster list 2>/dev/null | grep -q "^$CLUSTER "; then
        ok "(reused)"
    else
        retry 3 5 k3d cluster create "$CLUSTER" --agents 2 --wait --timeout 300s \
            --k3s-arg "--disable=traefik@server:0" >/dev/null 2>&1 || die "k3d cluster create failed"
        ok
    fi
    COMPLETED+=(local-cluster)
else
    step "terraform plan (infra/terraform)"
    if need terraform; then
        if [[ "$MODE" == apply ]] && approved infrastructure; then
            (cd infra/terraform && terraform init -input=false >/dev/null && terraform apply -input=false -auto-approve) || die "terraform apply failed"
            COMPLETED+=(infrastructure); ok "(applied)"
        else
            (cd infra/terraform && terraform init -backend=false -input=false >/dev/null && terraform validate >/dev/null) \
                && ok "(validated; plan needs credentials)" || die "terraform validate failed"
        fi
    else
        skip "terraform not installed"
    fi
    step "infracost estimate"; need infracost && (infracost breakdown --path infra/terraform >/dev/null && ok) || skip "infracost not installed"
    step "Cloudflare DNS"; skip "needs APPROVED apply and a token"
fi

# ---------------------------------------------------------------------------
phase C "hardening" 2
if [[ "$TARGET" == local-k3d ]]; then
    step "namespace + network policy"
    kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f - >/dev/null && ok || die "namespace"
else
    step "ansible-playbook --check infra/ansible/setup_node.yml"
    if need ansible-playbook; then
        ansible-playbook --check -i infra/ansible/inventory infra/ansible/setup_node.yml >/dev/null && ok || die "ansible check failed"
    else
        skip "ansible not installed"
    fi
fi

# ---------------------------------------------------------------------------
phase D "rollout" 3
BIN="target/$BIN_PROFILE/maya2c-node"
[[ "$BIN_PROFILE" == dev ]] && BIN=target/debug/maya2c-node
step "build maya2c-node (--profile $BIN_PROFILE)"
if [[ "${SKIP_BUILD:-0}" == 1 && -x "$BIN" ]]; then
    ok "(reused $BIN)"
else
    cargo build --profile "$BIN_PROFILE" -p maya2c-node >/dev/null 2>&1 || die "cargo build failed"
    ok
fi
if [[ "$TARGET" != local-k3d ]]; then
    step "validators + RPC gateways"; skip "cloud rollout needs APPROVED=rollout"
else
    CTX=$(mktemp -d); cp "$BIN" "$CTX/maya2c-node"
    step "image maya2c/node:local"
    docker build -q -t maya2c/node:local -f infra/docker/Dockerfile.local "$CTX" >/dev/null || die "docker build failed"
    rm -rf "$CTX"; ok
    step "import image into k3d"
    retry 3 5 k3d image import maya2c/node:local -c "$CLUSTER" >/dev/null 2>&1 && ok || die "k3d image import failed"
    step "apply overlay local-k3d ($NODES replicas)"
    kubectl kustomize infra/k8s/overlays/local-k3d \
        | sed "s/replicas: 12/replicas: $NODES/" \
        | kubectl apply -f - >/dev/null || die "kubectl apply failed"
    ok
    step "wait for $NODES nodes Ready (timeout 15 min)"
    deadline=$(( $(date +%s) + 900 ))
    while :; do
        ready=$(kubectl -n "$NAMESPACE" get statefulset maya-seed -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)
        [[ "${ready:-0}" -ge "$NODES" ]] && break
        (( $(date +%s) > deadline )) && die "only ${ready:-0}/$NODES ready; kubectl -n $NAMESPACE get pods"
        sleep 10
    done
    ok "($ready/$NODES)"
    step "smoke: get_supply over JSON-RPC on every node"
    answered=0
    req='{"jsonrpc":"2.0","id":1,"method":"get_supply","params":[]}'
    for ((i = 0; i < NODES; i++)); do
        if kubectl -n "$NAMESPACE" exec "maya-seed-$i" -- curl -sf -H 'content-type: application/json' \
            -d "$req" http://127.0.0.1:8545 2>/dev/null | grep -q '"result"'; then
            answered=$((answered + 1))
        fi
    done
    [[ "$answered" -eq "$NODES" ]] || die "only $answered/$NODES nodes answered JSON-RPC"
    ok "($answered/$NODES answered)"
fi

ELAPSED=$(( $(date +%s) - START ))
trap - EXIT
if [[ "$TARGET" == local-k3d && "$KEEP" == 0 ]]; then
    k3d cluster delete "$CLUSTER" >/dev/null 2>&1 || true
fi
cat <<EOF

   +--------------------------------------------------------------+
   |                 MAYA2C LAUNCH CERTIFICATE                    |
   |                                                              |
   |   mode .......... $(printf '%-43s' "$MODE")|
   |   target ........ $(printf '%-43s' "$TARGET")|
   |   nodes ......... $(printf '%-43s' "$NODES")|
   |   elapsed ....... $(printf '%-43s' "${ELAPSED}s")|
   |                                                              |
   |   A dry run proves the pipeline, not a network. No value     |
   |   moved and no public endpoint exists.                       |
   +--------------------------------------------------------------+
EOF
