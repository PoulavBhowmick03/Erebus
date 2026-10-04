FROM rust:1-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev clang cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY sdk/core sdk/core
COPY sdk/journal sdk/journal
COPY sdk/coordinator sdk/coordinator
COPY sdk/transport sdk/transport
COPY sdk/evm sdk/evm
COPY sdk/shielded sdk/shielded
RUN cargo build --locked --release --manifest-path sdk/transport/Cargo.toml --bin erebus-relay
RUN cargo build --locked --release --manifest-path sdk/shielded/Cargo.toml --bin erebus-access-service

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates caddy python3 && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/sdk/transport/target/release/erebus-relay /usr/local/bin/erebus-relay
COPY --from=build /build/sdk/shielded/target/release/erebus-access-service /usr/local/bin/erebus-access-service
COPY packaging/metropolis/render/run.py /usr/local/bin/erebus-render
ENTRYPOINT ["python3", "/usr/local/bin/erebus-render"]
