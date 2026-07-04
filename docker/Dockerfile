# agentd — multi-stage build: cargo → minimal runtime.
FROM rust:1.85-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml ./
COPY crates ./crates
COPY fixtures ./fixtures
RUN cargo build --release -p agentd-api

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 agentd
USER agentd
COPY --from=builder /build/target/release/agentd /usr/local/bin/agentd
ENV AGENTD_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/agentd"]
