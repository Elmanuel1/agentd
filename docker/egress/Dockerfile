# agentd-egress: the egress proxy as its own service. The runtime image is
# distroless: no shell, no package manager, only the binary and its libraries,
# run as the unprivileged `nonroot` user. Run it with a read-only root
# filesystem; it writes nothing.
#
#   docker build -f docker/egress/Dockerfile -t agentd-egress .
FROM rust:1.94-slim-bookworm AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY conformance ./conformance
RUN cargo build --release --locked -p egress

# Debian 12 in both stages, so the binary links against the glibc it runs with.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /build/target/release/agentd-egress /usr/local/bin/agentd-egress
USER nonroot:nonroot
EXPOSE 8890 8891
ENTRYPOINT ["/usr/local/bin/agentd-egress"]
