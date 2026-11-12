# syntax=docker/dockerfile:1

# Multi-stage builder image for imgforge (Rust stable + Node 22 LTS).
# Debian Bookworm (glibc 2.36) is intentionally chosen so compiled native Linux
# binaries execute directly on Ubuntu 24.04 (glibc 2.39) WSL2 hosts without
# host-installed Rust or Node toolchains.

FROM node:22.23.5-bookworm-slim@sha256:e9f565d32859f0cbfb52125eafe8be39390f24dac302dc3cdf566e8ba3ab21ee AS node-toolchain

FROM rust:1.100.0-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS builder

ARG BUILDER_UID=1000
ARG BUILDER_GID=1000

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/cache/cargo \
    npm_config_cache=/cache/npm \
    HOME=/tmp \
    PATH="/usr/local/cargo/bin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

# System C toolchain and TLS certificates required by Cargo build scripts.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        git \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Install Node 22 LTS and npm from the pinned Node stage without clobbering /usr/local/cargo.
COPY --from=node-toolchain /usr/local/bin/node /usr/local/bin/node
COPY --from=node-toolchain /usr/local/include/node /usr/local/include/node
COPY --from=node-toolchain /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -sf ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -sf ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

# Install Rust linting and formatting components, and make toolchains world-readable
# so any host UID/GID passed via `docker run --user` can invoke cargo/clippy/rustfmt.
RUN rustup component add clippy rustfmt \
    && chmod -R a+rX /usr/local/rustup /usr/local/cargo

# Create an unprivileged default builder user and writable cache directories.
RUN if ! getent group "${BUILDER_GID}" >/dev/null; then \
        groupadd --gid "${BUILDER_GID}" builder; \
    fi \
    && if ! id -u "${BUILDER_UID}" >/dev/null 2>&1; then \
        useradd --uid "${BUILDER_UID}" --gid "${BUILDER_GID}" --create-home --shell /bin/bash builder; \
    fi \
    && mkdir -p /workspace /cache/cargo /cache/npm \
    && chmod 1777 /cache /cache/cargo /cache/npm

# Verify pinned toolchain versions and Bookworm glibc compatibility (<= 2.39).
RUN rustc --version \
    && cargo --version \
    && cargo clippy --version \
    && cargo fmt --version \
    && node --version \
    && npm --version \
    && ldd --version | head -n 1

WORKDIR /workspace
USER ${BUILDER_UID}:${BUILDER_GID}

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD rustc --version >/dev/null 2>&1 && node --version >/dev/null 2>&1
