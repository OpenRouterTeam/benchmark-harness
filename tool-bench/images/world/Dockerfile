# syntax=docker/dockerfile:1.7
# tool-bench world image: every mock API + admin + TLS redirect, plus all scenario datasets.
# Build from the repo root:  docker build -f images/world/Dockerfile -t tool-bench-world .
ARG RUST_IMAGE=rust:1.99-bookworm
FROM ${RUST_IMAGE} AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    find crates -name '*.rs' -exec touch {} + \
 && cargo build --release --locked -p world && cp target/release/world /world

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /world /usr/local/bin/world
COPY scenarios /opt/tool-bench/scenarios
EXPOSE 443 8101-8130 9000
HEALTHCHECK --interval=1s --timeout=2s --retries=30 CMD curl -fs http://localhost:9000/_bench/health || exit 1
ENTRYPOINT ["world"]
