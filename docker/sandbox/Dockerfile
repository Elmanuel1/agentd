# The session sandbox. Its first process is the session's harness, the agent
# loop: it takes the session's work item from agentd, runs the agent with
# harness-sim's mini-swe-agent loop, and runs the agent's tools as `nobody`.
# agentd-worker here is the gateway relay agentd starts when it runs on the
# host. Build from the repository root:
#   docker build -f docker/sandbox/Dockerfile -t agentd-sandbox .
FROM rust:1.94-slim-bookworm AS worker
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY conformance ./conformance
RUN cargo build --release --locked -p sandbox-worker

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       bash ca-certificates coreutils curl file findutils grep jq less procps sed unzip zip \
       python3 python3-pip python3-pypdf poppler-utils \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /workspace /mnt/session/uploads /mnt/session/outputs \
    && chown 65534:65534 /workspace /mnt/session/outputs
COPY harness /opt/agentd-harness
RUN pip install --no-cache-dir --break-system-packages /opt/agentd-harness
COPY --from=worker /build/target/release/agentd-worker /usr/local/bin/agentd-worker
ENV HOME=/workspace
WORKDIR /workspace
CMD ["python3", "-m", "agentd_harness"]
