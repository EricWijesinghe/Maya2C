# One staging validator per container, built around host-built binaries.
#
# Same base as infra/docker/Dockerfile.local, and for the same reason: the
# binaries are linked against the build host's glibc (Ubuntu 24.04, 2.39).
# iproute2 is here for `tc netem`: the entrypoint gives each container its own
# WAN-like delay and loss before the node starts, so seven containers on one
# PC exchange traffic the way seven distant machines would. It is still one
# PC — see README.md.
FROM ubuntu:24.04
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libstdc++6 curl iproute2 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 10001 l1 && useradd -u 10001 -g l1 -d /home/l1 -m l1 \
    && mkdir -p /data && chown -R l1:l1 /data
COPY maya2c-node /usr/local/bin/maya2c-node
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
EXPOSE 8545 30333
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
