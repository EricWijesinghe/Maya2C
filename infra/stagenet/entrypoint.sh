#!/bin/sh
# Applies this host's simulated WAN link, then drops root and starts the node.
# NETEM is passed to `tc qdisc ... netem` verbatim, e.g. "delay 80ms 15ms loss 0.5%".
# tc needs CAP_NET_ADMIN, which compose grants; the node itself never runs as root.
set -eu
if [ -n "${NETEM:-}" ]; then
    # shellcheck disable=SC2086 # NETEM is a list of tc arguments
    tc qdisc replace dev eth0 root netem $NETEM
    echo "stagenet: eth0 netem $NETEM"
fi
chown -R l1:l1 /data
exec setpriv --reuid=10001 --regid=10001 --init-groups /usr/local/bin/maya2c-node "$@"
