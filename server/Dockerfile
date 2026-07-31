# Build context is the REPO ROOT, not this directory — the image needs core/,
# server/ and data/. Build with:
#   docker build -f server/Dockerfile .

# ---- Stage 1: dependency cache -------------------------------------------
# Compiles a stub crate so the slow, LTO'd dependency build lands in a layer
# that only invalidates when a Cargo.toml or Cargo.lock changes.
FROM rust:1.97-alpine AS cacher
WORKDIR /build
# musl-dev/gcc/make/perl are for `ring`, which reqwest's rustls-tls pulls in.
# No openssl or pkgconfig: this build has no native TLS dependency.
RUN apk --no-cache upgrade && \
    apk add --no-cache musl-dev gcc make perl
COPY core/Cargo.toml core/Cargo.toml
COPY server/Cargo.toml server/Cargo.lock server/
RUN mkdir -p core/src server/src && \
    : > core/src/lib.rs && \
    echo 'fn main() {}' > server/src/main.rs && \
    cd server && cargo build --release --locked && \
    rm -rf target/release/deps/*traintime_core* \
           target/release/deps/*traintime_server* \
           target/release/.fingerprint/traintime-core-* \
           target/release/.fingerprint/traintime-server-* \
           target/release/traintime-server

# ---- Stage 2: build ------------------------------------------------------
FROM rust:1.97-alpine AS builder
WORKDIR /build
RUN apk --no-cache upgrade && \
    apk add --no-cache musl-dev gcc make perl
COPY --from=cacher /build/server/target server/target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
COPY core core
COPY server server
RUN cd server && cargo build --release --locked

# ---- Stage 3: runtime ----------------------------------------------------
FROM alpine:3.21
RUN apk --no-cache upgrade && \
    apk add --no-cache ca-certificates tzdata libgcc && \
    adduser -D -u 10001 traintime
WORKDIR /app
COPY --from=builder /build/server/target/release/traintime-server /usr/local/bin/traintime-server
# Own layer, so the monthly station-data commit rebuilds only this.
COPY data/stations.json /app/stations.json
ENV PORT=8080 \
    STATIONS_PATH=/app/stations.json
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/traintime-server"]
