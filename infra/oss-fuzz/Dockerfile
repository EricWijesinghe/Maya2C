# Builder image with the latest nightly Rust and cargo-fuzz on PATH.
FROM gcr.io/oss-fuzz-base/base-builder-rust

# Fetched at build time, as OSS-Fuzz expects. Pin a commit once the fuzz
# targets are stable so a build is reproducible.
RUN git clone --depth 1 https://github.com/EricWijesinghe/Maya2C.git maya2c
WORKDIR maya2c

COPY build.sh $SRC/
