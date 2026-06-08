# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# Production image for the `ferro-cargo-registry-server` — a Cargo sparse
# index + crate publish (`/api/v1/crates/new`) registry server.
#
# Build:
#   docker build -f deploy/ferro-cargo-registry-server/Dockerfile \
#       -t ferro-cargo-registry-server:0.1.0 .
# Multi-arch (linux/amd64 + linux/arm64):
#   docker buildx build --platform linux/amd64,linux/arm64 \
#       -f deploy/ferro-cargo-registry-server/Dockerfile \
#       -t ghcr.io/abyo-software/ferro-cargo-registry-server:0.1.0 --push .
# Run:
#   docker run --rm -p 8081:8081 \
#       -e FERRO_CARGO_REGISTRY_DATA=/var/lib/ferro-cargo-registry \
#       -v ferro-cargo-data:/var/lib/ferro-cargo-registry \
#       ferro-cargo-registry-server:0.1.0
#
# Image philosophy:
#   * Multi-stage: Rust toolchain pinned in builder, runtime is debian-slim.
#   * Non-root by default (UID 65534 / nobody — present in both bookworm-slim
#     and distroless bases).
#   * `cargo build --locked` for deterministic Cargo.lock resolution.
#   * Workspace release profile already strips debug syms + thin LTO +
#     codegen-units=1; this Dockerfile reinforces with a final `strip` pass.
#   * The server persists the sparse index + crate blobs to
#     FERRO_CARGO_REGISTRY_DATA (defaults to ./registry-data). This image
#     points it at a VOLUME so a plain `docker run` is durable out of the box.

ARG RUST_VERSION=1.91
ARG BASE_IMAGE=debian:bookworm-slim

# ---- Builder stage -------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /workspace

# Build deps. The axum/tokio stack here uses rustls (no openssl), so
# pkg-config / libssl are not strictly required, but pkg-config keeps a
# few transitive crates happy on first build. ca-certificates lets cargo
# fetch crates if the build network-hops despite --locked (it shouldn't).
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests + sources. A `crates/**/src/**` edit busts the
# build layer but the cache mounts below keep incremental rebuilds fast.
COPY Cargo.toml Cargo.lock ./
COPY crates crates

# BuildKit cache mounts for the registry + target dirs so incremental
# rebuilds during local iteration stay fast.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target \
    cargo build \
        --release \
        --locked \
        --bin ferro-cargo-registry-server \
    && cp /workspace/target/release/ferro-cargo-registry-server /usr/local/bin/ferro-cargo-registry-server \
    && strip /usr/local/bin/ferro-cargo-registry-server

# ---- Runtime stage -------------------------------------------------------
FROM ${BASE_IMAGE} AS runtime

# Runtime deps:
#   * ca-certificates — TLS trust store for any HTTPS upstream fetches.
#   * tini — PID 1 / SIGTERM forwarding. axum's serve handles graceful
#     shutdown, but tini guards against PID1 zombies when operators exec
#     into the container.
#   * wget — used by the HEALTHCHECK to probe /healthz.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
        wget \
    && rm -rf /var/lib/apt/lists/*

# Registry data directory (sparse index + crate blobs). Owned by the
# non-root runtime UID so the server can create files without a
# root-owned mount fighting it.
RUN mkdir -p /var/lib/ferro-cargo-registry \
    && chown -R 65534:65534 /var/lib/ferro-cargo-registry

# Non-root user. 65534 == `nobody` on Debian.
USER 65534:65534

WORKDIR /var/lib/ferro-cargo-registry

COPY --from=builder /usr/local/bin/ferro-cargo-registry-server /usr/local/bin/ferro-cargo-registry-server

# Persist the index + crates to the data dir by default; mark it a VOLUME
# so a plain `docker run` survives container recreation.
ENV FERRO_CARGO_REGISTRY_DATA=/var/lib/ferro-cargo-registry \
    FERRO_CARGO_REGISTRY_LISTEN=0.0.0.0:8081
VOLUME ["/var/lib/ferro-cargo-registry"]

# Healthcheck hits /healthz on port 8081 ({"status":"ok"}).
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD wget -q -O- http://127.0.0.1:8081/healthz 2>/dev/null \
        | grep -q '"status":"ok"' || exit 1

# Sparse index + publish API + probe endpoints listen on 8081.
EXPOSE 8081

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ferro-cargo-registry-server"]

# Image metadata (OCI labels — picked up by registries, Docker Hub UI, etc.)
LABEL org.opencontainers.image.title="ferro-cargo-registry-server" \
      org.opencontainers.image.description="Rust-native Cargo sparse-index + crate publish registry server" \
      org.opencontainers.image.source="https://github.com/abyo-software/ferro-protocols" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="0.1.0" \
      org.opencontainers.image.vendor="abyo software 合同会社 (abyo software LLC)"
