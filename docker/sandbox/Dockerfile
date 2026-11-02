# The session sandbox: where an agent's tools run. agentd keeps the model
# keys and runs the loop; this image only needs what tools call: a shell,
# the usual text tools, Python 3 and PDF tools.
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       bash ca-certificates coreutils curl file findutils grep jq less procps sed unzip zip \
       python3 python3-pip python3-pypdf poppler-utils \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /workspace /mnt/session/uploads /mnt/session/outputs \
    && chown 65534:65534 /workspace /mnt/session/outputs
ENV HOME=/workspace
WORKDIR /workspace
USER 65534:65534
CMD ["sleep", "infinity"]
