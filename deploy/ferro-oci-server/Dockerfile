# syntax=docker/dockerfile:1.7
# SPDX-License-Identifier: Apache-2.0
#
# Production image for the `ferro-oci-server` — an OCI Distribution
# (`/v2/`) registry server.
#
# Build:
#   docker build -f deploy/ferro-oci-server/Dockerfile -t ferro-oci-server:0.1.0 .
# Multi-arch (linux/amd64 + linux/arm64):
#   docker buildx build --platform linux/amd64,linux/arm64 \
#       -f deploy/ferro-oci-server/Dockerfile \
#       -t ghcr.io/abyo-software/ferro-oci-server:0.1.0 --push .
# Run:
#   docker run --rm -p 8080:8080 \
#       -e FERRO_OCI_STORAGE_DIR=/var/lib/ferro-oci-server \
#       -v ferro-oci-data:/var/lib/ferro-oci-server \
#       ferro-oci-server:0.1.0
#
# Image philosophy:
#   * Multi-stage: Rust toolchain pinned in builder, runtime is debian-slim.
#   * Non-root by default (UID 65534 / nobody — present in both bookworm-slim
#     and distroless bases).
#   * `cargo build --locked` for deterministic Cargo.lock resolution.
#   * Workspace release profile already strips debug syms + thin LTO +
#     codegen-units=1; this Dockerfile reinforces with a final `strip` pass.
#   * The server persists blob bytes to FERRO_OCI_STORAGE_DIR (defaults to an
#     in-memory store when unset). This image points it at a VOLUME so a
#     plain `docker run` is durable out of the box.

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
        --bin ferro-oci-server \
    && cp /workspace/target/release/ferro-oci-server /usr/local/bin/ferro-oci-server \
    && strip /usr/local/bin/ferro-oci-server

# ---- Runtime stage -------------------------------------------------------
FROM ${BASE_IMAGE} AS runtime

# Runtime deps:
#   * ca-certificates — TLS trust store (e.g. referrer / cross-repo mounts
#     over HTTPS to upstreams).
#   * tini — PID 1 / SIGTERM forwarding. axum's serve handles graceful
#     shutdown, but tini guards against PID1 zombies when operators exec
#     into the container.
#   * wget — used by the HEALTHCHECK to probe /healthz.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
        wget \
    && rm -rf /var/lib/apt/lists/*

# Blob-store data directory. Owned by the non-root runtime UID so the
# server can create blobs without a root-owned mount fighting it.
RUN mkdir -p /var/lib/ferro-oci-server \
    && chown -R 65534:65534 /var/lib/ferro-oci-server

# Non-root user. 65534 == `nobody` on Debian.
USER 65534:65534

WORKDIR /var/lib/ferro-oci-server

COPY --from=builder /usr/local/bin/ferro-oci-server /usr/local/bin/ferro-oci-server

# Persist blobs to the data dir by default; mark it a VOLUME so a plain
# `docker run` survives container recreation.
ENV FERRO_OCI_STORAGE_DIR=/var/lib/ferro-oci-server \
    FERRO_OCI_LISTEN=0.0.0.0:8080
VOLUME ["/var/lib/ferro-oci-server"]

# Healthcheck hits /healthz on port 8080 ({"status":"ok"}).
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD wget -q -O- http://127.0.0.1:8080/healthz 2>/dev/null \
        | grep -q '"status":"ok"' || exit 1

# OCI Distribution + probe endpoints listen on 8080.
EXPOSE 8080

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ferro-oci-server"]

# Image metadata (OCI labels — picked up by registries, Docker Hub UI, etc.)
LABEL org.opencontainers.image.title="ferro-oci-server" \
      org.opencontainers.image.description="Rust-native OCI Distribution (/v2/) registry server" \
      org.opencontainers.image.source="https://github.com/abyo-software/ferro-protocols" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="0.1.0" \
      org.opencontainers.image.vendor="abyo software 合同会社 (abyo software LLC)"
