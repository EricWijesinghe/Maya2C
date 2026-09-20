# syntax=docker/dockerfile:1
#
# Multi-stage build for custom-l1-node.
#
# Stage 1 compiles against musl on Alpine; stage 2 is a minimal Alpine runtime
# holding only the binaries and a non-root user.
#
# ## On "static"
#
# Rust code targeting musl links statically by default. The complication is
# RocksDB: it is C++, so the binary also needs the C++ runtime. This build links
# libstdc++ and libgcc statically to produce a genuinely self-contained binary.
#
# If that link step fails on your toolchain — it is the fragile part of this
# build, and depends on the Alpine/LLVM versions in play — switch to the
# dynamic fallback documented at the bottom of this file. It produces a slightly
# larger image that is considerably more reliable.

# ---------------------------------------------------------------------------
# Stage 1: build
# ---------------------------------------------------------------------------
# Pin to an exact patch version for a real deployment; `1-alpine` floats and
# will silently change your build inputs.
FROM rust:1-alpine AS builder

# build-base   : gcc/g++/make, and the static libstdc++.a we link against
# clang-dev    : libclang, required by bindgen inside librocksdb-sys
# linux-headers: kernel headers RocksDB includes
# Nothing here pulls OpenSSL: jsonrpsee and libp2p both use rustls.
RUN apk add --no-cache \
        build-base \
        clang-dev \
        llvm-dev \
        linux-headers \
        git

# bindgen locates libclang through this.
ENV LIBCLANG_PATH=/usr/lib

# Static C++ runtime, so the result does not need libstdc++ at runtime.
ENV RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-static-libstdc++ -C link-arg=-static-libgcc"

WORKDIR /build

# --- dependency cache layer ------------------------------------------------
# RocksDB alone is several minutes of C++ compilation. Building it against stub
# sources first means editing node code does not rebuild the whole dependency
# tree.
# The whole workspace's manifests, so cargo can resolve without the sources.
# There is no shorter way: this is a virtual workspace and cargo needs every
# manifest named in `members` to exist before it will plan a build. The
# stub-source trick this file used before 2026-09-20 depended on the binaries
# being `[[bin]]` targets of a root package, which they no longer are.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo/config.toml .cargo/config.toml
COPY crates crates
COPY bins bins
COPY apps apps
COPY hal hal
COPY sdks sdks
COPY sim sim
COPY xtask xtask

# `.cargo/config.toml` pins `jobs = 4` for a 32 GB developer machine whose
# commit limit a wider build exhausts. A build container should use what it
# has.
ENV CARGO_BUILD_JOBS=0

RUN cargo build --release         --bin maya2c-node --bin maya2c-genesis --bin maya2c-peerid     && strip target/release/maya2c-node              target/release/maya2c-genesis              target/release/maya2c-peerid

# Fail the build rather than ship a binary that will not start in the runtime
# stage, where no C++ runtime is installed.
RUN if ldd target/release/maya2c-node 2>&1 | grep -q "libstdc++"; then \
        echo "ERROR: maya2c-node still links libstdc++ dynamically" >&2; exit 1; \
    fi

# ---------------------------------------------------------------------------
# Stage 2: runtime
# ---------------------------------------------------------------------------
FROM alpine:3 AS runtime

# ca-certificates for any future outbound TLS; busybox already supplies the
# wget used by the compose healthcheck.
RUN apk add --no-cache ca-certificates \
    && addgroup -S l1 \
    && adduser -S -G l1 -h /home/l1 l1 \
    && mkdir -p /data /config \
    && chown -R l1:l1 /data /config

COPY --from=builder /build/target/release/maya2c-node    /usr/local/bin/maya2c-node
COPY --from=builder /build/target/release/maya2c-genesis /usr/local/bin/maya2c-genesis
# Used by the seed identity init container and by operators pre-generating the
# PeerIds that cross-region bootnode multiaddrs are written against.
COPY --from=builder /build/target/release/maya2c-peerid  /usr/local/bin/maya2c-peerid

# Never run a network daemon as root.
USER l1

# State lives here; mount a volume so it survives container replacement.
VOLUME ["/data"]

# 8545 JSON-RPC, 8546 aggregator market/supply HTTP, 9600 Prometheus exporter,
# 30333 libp2p TCP.
#
# EXPOSE is documentation, not a firewall. Only 30333 and 8545 are meant to be
# reachable from outside a cluster; 9600 publishes peer topology and mempool
# contents and is confined by the NetworkPolicy in infra/k8s/base/ingress.yaml.
EXPOSE 8545 8546 9600 30333

ENV L1_DATA_DIR=/data \
    L1_GENESIS=/config/genesis.json

ENTRYPOINT ["/usr/local/bin/maya2c-node"]
# The exporter is on by default because the container's health checks target
# /healthz, which it serves. The market endpoint is not: it is only useful once
# an operator supplies a feed, and its supply scan is the most expensive read
# the node offers.
CMD ["--genesis", "/config/genesis.json", \
     "--data-dir", "/data", \
     "--rpc-addr", "0.0.0.0:8545", \
     "--metrics-addr", "0.0.0.0:9600", \
     "--p2p-port", "30333"]

# ---------------------------------------------------------------------------
# Dynamic fallback
# ---------------------------------------------------------------------------
# If the static C++ link fails, make two changes:
#
#   1. In the builder stage:
#        ENV RUSTFLAGS=""
#      and delete the `ldd` guard above.
#   2. In the runtime stage:
#        RUN apk add --no-cache ca-certificates libstdc++ libgcc
#
# The image grows by a few megabytes and gains a runtime dependency on
# libstdc++, but the build is far less sensitive to toolchain versions.
