#!/usr/bin/env bash
#
# A veth pair with one end in a network namespace, for `crates/node/benches/ebpf_bench.rs`.
#
# Usage:
#   sudo ./scripts/xdp_netns.sh up
#   sudo -E MAYA_XDP_BENCH=1 MAYA_XDP_OBJECT=... cargo bench --features xdp --bench ebpf_bench
#   sudo ./scripts/xdp_netns.sh down
#
# The host end (maya-xdp0, 10.201.0.1) is where the XDP program attaches; the
# peer end (maya-xdp1, 10.201.0.2) lives in namespace maya-xdp-peer, where the
# benchmark's traffic generator threads send from.
#
# What a veth pair can and cannot show: XDP runs in generic mode on veth unless
# the peer end also has a program, and AF_XDP binds in copy mode. The numbers
# are a property of this path on this machine — the drop comparison holds up
# well on veth, the receive comparison much less so. A NIC with native XDP and
# zero-copy support (mlx5, ice, i40e) is what measures the receive path.

set -euo pipefail

readonly NAMESPACE="${MAYA_XDP_NETNS:-maya-xdp-peer}"
readonly HOST_IF="${MAYA_XDP_IFACE:-maya-xdp0}"
readonly PEER_IF="${MAYA_XDP_PEER_IFACE:-maya-xdp1}"
readonly HOST_ADDR="${MAYA_XDP_LOCAL:-10.201.0.1}"
readonly PEER_ADDR="${MAYA_XDP_PEER:-10.201.0.2}"

up() {
    ip netns add "$NAMESPACE"
    ip link add "$HOST_IF" type veth peer name "$PEER_IF"
    ip link set "$PEER_IF" netns "$NAMESPACE"
    ip addr add "$HOST_ADDR/24" dev "$HOST_IF"
    ip link set "$HOST_IF" up
    ip netns exec "$NAMESPACE" ip addr add "$PEER_ADDR/24" dev "$PEER_IF"
    ip netns exec "$NAMESPACE" ip link set "$PEER_IF" up
    ip netns exec "$NAMESPACE" ip link set lo up
    echo "up: $HOST_IF ($HOST_ADDR) <-> $NAMESPACE/$PEER_IF ($PEER_ADDR)"
}

down() {
    # Deleting one end of a veth pair deletes both. Either may already be gone.
    ip link del "$HOST_IF" 2>/dev/null || true
    ip netns del "$NAMESPACE" 2>/dev/null || true
    echo "down"
}

case "${1:-}" in
    up) up ;;
    down) down ;;
    *)
        echo "usage: $0 up|down" >&2
        exit 2
        ;;
esac
