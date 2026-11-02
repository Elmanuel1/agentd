# The session sandbox: where an agent's tools run. agentd keeps the model
# keys and runs the loop; the sandbox's first process is agentd-worker,
# which takes the session's operations from agentd (it opens no port) and
# runs each command as `nobody`. Build from the repository root:
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
COPY --from=worker /build/target/release/agentd-worker /usr/local/bin/agentd-worker
ENV HOME=/workspace
WORKDIR /workspace
CMD ["agentd-worker"]
