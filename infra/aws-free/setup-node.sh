#!/bin/bash
# One validator host of infra/aws-free, run as root through SSM RunCommand.
#
#   setup-node.sh keys  <N> <bucket>   install binaries, make and seal the key
#   setup-node.sh start <N> <bucket>   install genesis and units, start
#
# Two phases because genesis names every validator's public key: all four
# hosts run `keys`, the operator builds genesis from public/, then all four
# run `start`. Only public material is uploaded: the public key and peer id.
set -euo pipefail

phase=${1:?phase}; n=${2:?validator number}; bucket=${3:?bucket}
state=/var/lib/maya2c; conf=/etc/maya2c; bin=/usr/local/bin

log() { printf '[aws-free v%s] %s\n' "$n" "$*"; }

# IMDSv2 credentials through the AWS CLI that AL2023 ships with.
fetch() { aws s3 cp --only-show-errors "s3://$bucket/$1" "$2"; }
put() { aws s3 cp --only-show-errors "$1" "s3://$bucket/$2"; }

install_binaries() {
    local tmp; tmp=$(mktemp -d)
    fetch release/maya2c-aarch64-unknown-linux-gnu.tar.gz "$tmp/node.tgz"
    tar -xzf "$tmp/node.tgz" -C "$tmp"
    install -m 0755 "$tmp"/maya2c-aarch64-unknown-linux-gnu/{maya2c-node,maya2c-peerid,l1-wallet} "$bin/"
    rm -rf "$tmp"
}

keys() {
    install_binaries
    if [ -f "$state/validator.key.sealed" ]; then
        log "key already sealed; reusing it"
    else
        # Root-only; systemd hands it to the node as a credential.
        (umask 077; head -c 32 /dev/urandom | base64 > "$conf/validator.passphrase")
        # Written sealed from the start: no plaintext key ever touches disk.
        "$bin/maya2c-node" --validator-passphrase-file "$conf/validator.passphrase" \
            --generate-validator-key "$state/validator.key.sealed" \
            | tr -d '[:space:]' > "$state/validator.pub"
        chown maya:maya "$state/validator.key.sealed" "$state/validator.pub"
        chmod 0400 "$state/validator.key.sealed"
    fi
    # v1 keeps the PC seed's p2p identity (uploaded to secret/node_key), so the
    # published bootnode address stays valid.
    install -d -o maya -g maya -m 0700 "$state/data"
    if [ "$n" = 1 ] && [ ! -f "$state/data/node_key" ]; then
        fetch secret/node_key "$state/data/node_key"
        aws s3 rm --only-show-errors "s3://$bucket/secret/node_key"
        chown maya:maya "$state/data/node_key"; chmod 0400 "$state/data/node_key"
    fi
    local peer
    peer=$(runuser -u maya -- "$bin/maya2c-peerid" --data-dir "$state/data" --create | tail -1)
    printf '%s\n' "$peer" > "$state/peer.id"
    put "$state/validator.pub" "public/v$n.pub"
    put "$state/peer.id" "public/v$n.peer"
    log "public key and peer id uploaded"
}

sandbox() { # $1 = writable path
    cat <<EOF
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$1
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources
SystemCallErrorNumber=EPERM
ProtectProc=invisible
ProcSubset=pid
RemoveIPC=yes
CapabilityBoundingSet=
AmbientCapabilities=
UMask=0077
EOF
}

start() {
    fetch release/genesis.json "$conf/genesis.json"
    fetch release/node-config.toml "$conf/node-config.toml"
    fetch release/bootnodes.txt "$conf/bootnodes.txt"
    # Every other validator, by private IP and peer id.
    local boot="" rpc=0.0.0.0:8545 extra=""
    while read -r idx addr; do
        [ "$idx" = "$n" ] || boot="$boot --bootnode $addr"
    done < "$conf/bootnodes.txt"
    # v1's RPC serves only its own gateway and status page.
    if [ "$n" = 1 ]; then
        rpc=127.0.0.1:8545
        extra="--ws-listen /ip4/127.0.0.1/tcp/31101/ws"
    fi
    cat > /etc/systemd/system/maya2c-validator.service <<EOF
[Unit]
Description=maya-testnet-1 validator v$n
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=maya
Group=maya
LoadCredential=passphrase:$conf/validator.passphrase
ExecStart=$bin/maya2c-node --genesis $conf/genesis.json --config $conf/node-config.toml --data-dir $state/data --rpc-addr $rpc --p2p-port 31100 --validator-key $state/validator.key.sealed --validator-passphrase-file \${CREDENTIALS_DIRECTORY}/passphrase --min-register-bond 1000000000 --snapshot-interval 3600 --snapshot-depth 3600 --prune-depth 3600 $extra$boot
Restart=always
RestartSec=5
LimitNOFILE=65536
$(sandbox "$state")

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable --now maya2c-validator.service
    log "validator started"
}

case "$phase" in
    keys) keys ;;
    start) start ;;
    # Shared with setup-services.sh, so every unit gets the same sandbox.
    sandbox) sandbox "${4:?writable path}" ;;
    *) echo "unknown phase $phase" >&2; exit 2 ;;
esac
