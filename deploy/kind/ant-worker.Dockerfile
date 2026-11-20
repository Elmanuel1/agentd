# The tool worker: Anthropic's `ant beta:worker poll` (the version CI pins), which long-polls a self_hosted
# environment and runs each session's tools in /workspace. deploy/kind/e2e.sh starts it for its environment.
#   docker build -f deploy/kind/ant-worker.Dockerfile -t agentd-ant-worker:kind deploy/kind
FROM debian:trixie-slim
ARG ANT_VERSION=1.39.0
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && curl -sfL "https://github.com/anthropics/anthropic-cli/releases/download/v${ANT_VERSION}/ant_${ANT_VERSION}_linux_${TARGETARCH}.tar.gz" \
       | tar -xz -C /usr/local/bin ant \
    && apt-get purge -y curl && apt-get autoremove -y && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 worker && mkdir /workspace && chown worker /workspace
USER worker
WORKDIR /workspace
ENTRYPOINT ["ant", "beta:worker", "poll", "--workdir", "/workspace", "--max-idle", "0"]
