# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.88-bookworm
ARG CURSOR_AGENT_VERSION=2026.06.19-20-24-33-653a7fb

FROM rust:${RUST_VERSION} AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p copenai-cli

FROM debian:bookworm-slim AS runtime
ARG CURSOR_AGENT_VERSION
ARG TARGETARCH

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN case "${TARGETARCH}" in \
        amd64) AGENT_ARCH=x64 ;; \
        arm64) AGENT_ARCH=arm64 ;; \
        *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
    && mkdir -p /opt/cursor-agent \
    && curl -fSL "https://downloads.cursor.com/lab/${CURSOR_AGENT_VERSION}/linux/${AGENT_ARCH}/agent-cli-package.tar.gz" \
        | tar --strip-components=1 -xzf - -C /opt/cursor-agent \
    && ln -sf /opt/cursor-agent/cursor-agent /usr/local/bin/agent

COPY --from=builder /app/target/release/copenai /usr/local/bin/copenai

RUN groupadd -g 1000 copenai \
    && useradd -u 1000 -g copenai -d /data -s /bin/bash copenai \
    && mkdir -p /data \
    && chown copenai:copenai /data

ENV COPENAI_HOME=/data \
    HOME=/data \
    RUST_LOG=info

USER copenai
WORKDIR /data
EXPOSE 9241

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:9241/health >/dev/null || exit 1

ENTRYPOINT ["copenai", "--daemon"]
