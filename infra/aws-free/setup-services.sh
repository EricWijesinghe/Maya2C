#!/bin/bash
# The public services on v1, run as root through SSM after setup-node.sh start.
# Same ports and flags the PC seed used (D:\Maya2C-testnet\start-testnet.ps1),
# all on 127.0.0.1: the Cloudflare tunnel is the only way in.
#
#   setup-services.sh <bucket> <cloudflared-version>
#
# The tunnel credentials are uploaded by the owner to secret/tunnel.json and
# deleted from the bucket as soon as they are read.
set -euo pipefail

bucket=${1:?bucket}; cf_version=${2:?cloudflared version}
state=/var/lib/maya2c; conf=/etc/maya2c; bin=/usr/local/bin
here=$(cd "$(dirname "$0")" && pwd)
sandbox() { "$here/setup-node.sh" sandbox 1 "$bucket" "$1"; }

fetch() { aws s3 cp --only-show-errors "s3://$bucket/$1" "$2"; }

install_binaries() {
    local tmp; tmp=$(mktemp -d)
    fetch release/maya2c-services-aarch64-unknown-linux-gnu.tar.gz "$tmp/svc.tgz"
    tar -xzf "$tmp/svc.tgz" -C "$tmp"
    install -m 0755 "$tmp"/maya2c-services-aarch64-unknown-linux-gnu/* "$bin/"
    fetch release/explorer-assets.tar.gz "$tmp/assets.tgz"
    install -d -m 0755 /opt/maya2c/explorer-assets
    tar -xzf "$tmp/assets.tgz" -C /opt/maya2c/explorer-assets
    # The relay keeps the PC relay's libp2p key, so its address is unchanged.
    install -d -o maya -g maya -m 0700 "$state/relay"
    if fetch secret/relay.tar.gz "$tmp/relay.tgz" 2>/dev/null; then
        tar -xzf "$tmp/relay.tgz" -C "$state/relay"
        chown -R maya:maya "$state/relay"
        aws s3 rm --only-show-errors "s3://$bucket/secret/relay.tar.gz"
    fi
    rm -rf "$tmp"
}

unit() { # $1 name, $2 description, $3 command, $4 writable path
    cat > "/etc/systemd/system/$1.service" <<EOF
[Unit]
Description=$2
After=maya2c-validator.service
Wants=maya2c-validator.service

[Service]
Type=simple
User=maya
Group=maya
ExecStart=$3
Restart=always
RestartSec=5
$(sandbox "$4")

[Install]
WantedBy=multi-user.target
EOF
}

watch_flags() {
    local flags="" idx addr ip peer
    while read -r idx addr; do
        ip=$(cut -d/ -f3 <<< "$addr"); peer=${addr##*/}
        [ "$idx" = 1 ] && ip=127.0.0.1
        flags="$flags --watch v$idx=http://$ip:8545 --known v$idx=$peer"
    done < "$conf/bootnodes.txt"
    printf '%s' "$flags"
}

services() {
    local node=http://127.0.0.1:8545
    unit maya2c-gateway "maya-testnet-1 public RPC gateway" \
        "$bin/maya2c-gateway --node $node --listen 127.0.0.1:8080 --client-ip cf-connecting-ip" /tmp
    unit maya2c-bootstrap "maya-testnet-1 snapshot bootstrap" \
        "$bin/maya2c-gateway --node $node --listen 127.0.0.1:8081 --client-ip cf-connecting-ip --bootstrap --rate-per-second 10 --rate-burst 20" /tmp
    unit maya-explorer "maya-testnet-1 explorer" \
        "$bin/explorer --node-rpc $node --listen 127.0.0.1:3000 --assets /opt/maya2c/explorer-assets" /tmp
    unit maya-status "maya-testnet-1 status page" \
        "$bin/maya-status --node $node --listen 127.0.0.1:3100$(watch_flags)" /tmp
    unit maya-chat-relay "Maya Chat relay" \
        "$bin/maya-chat --home $state/relay relay --listen /ip4/127.0.0.1/tcp/4002/ws" "$state/relay"
}

tunnel() {
    dnf install -y -q "https://github.com/cloudflare/cloudflared/releases/download/$cf_version/cloudflared-linux-aarch64.rpm"
    if [ ! -f "$conf/tunnel.json" ]; then
        (umask 077; fetch secret/tunnel.json "$conf/tunnel.json")
        aws s3 rm --only-show-errors "s3://$bucket/secret/tunnel.json"
    fi
    local id; id=$(jq -r .TunnelID "$conf/tunnel.json")
    cat > "$conf/cloudflared.yml" <<EOF
tunnel: $id
credentials-file: $conf/tunnel.json
ingress:
  - { hostname: rpc.maya2c.dev, service: http://127.0.0.1:8080 }
  - { hostname: bootstrap.maya2c.dev, service: http://127.0.0.1:8081 }
  - { hostname: explorer.maya2c.dev, service: http://127.0.0.1:3000 }
  - { hostname: status.maya2c.dev, service: http://127.0.0.1:3100 }
  - { hostname: chat.maya2c.dev, service: http://127.0.0.1:4002 }
  - { hostname: p2p.maya2c.dev, service: http://127.0.0.1:31101 }
  - service: http_status:404
EOF
    cat > /etc/systemd/system/cloudflared-maya.service <<EOF
[Unit]
Description=Cloudflare tunnel for maya-testnet-1
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/bin/cloudflared --no-autoupdate tunnel --config $conf/cloudflared.yml run
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
}

install_binaries
services
tunnel
systemctl daemon-reload
systemctl enable --now maya2c-gateway maya2c-bootstrap maya-explorer maya-status maya-chat-relay cloudflared-maya
echo "[aws-free v1] services and tunnel started"
