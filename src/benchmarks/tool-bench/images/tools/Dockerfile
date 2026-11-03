# syntax=docker/dockerfile:1.7
# tool-bench tools image: every MCP server from catalog/services.toml at its pinned version.
# Build from the repo root:  docker build -f images/tools/Dockerfile -t tool-bench-tools .

ARG RUST_IMAGE=rust:1.99-bookworm
ARG GO_IMAGE=golang:1.27-bookworm
ARG NODE_IMAGE=node:24-bookworm-slim
ARG UV_IMAGE=ghcr.io/astral-sh/uv:0.12.10

# ── Our Rust binaries ────────────────────────────────────────────────────────
FROM ${RUST_IMAGE} AS rust-build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    find crates -name '*.rs' -exec touch {} + \
 && cargo build --release --locked -p mcp-tools -p catalog \
 && mkdir -p /out \
 && cp target/release/toolmeter target/release/mcp-bridge target/release/catalog target/release/tools-supervisor /out/

# ── Elasticsearch MCP server (Rust, from its git tag) ───────────────────────
FROM ${RUST_IMAGE} AS es-build
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo install --locked --root /out \
      --git https://github.com/elastic/mcp-server-elasticsearch --tag v0.4.6 \
      elasticsearch-core-mcp-server

# ── Go servers ───────────────────────────────────────────────────────────────
FROM ${GO_IMAGE} AS go-build
ENV GOBIN=/out GOTOOLCHAIN=auto CGO_ENABLED=0
RUN --mount=type=cache,target=/root/go/pkg/mod \
    go install github.com/github/github-mcp-server/v2/cmd/github-mcp-server@v2.0.3 \
 && go install github.com/grafana/mcp-grafana/v2/cmd/mcp-grafana@v2.1.1 \
 && go install github.com/hashicorp/terraform-mcp-server/cmd/terraform-mcp-server@v1.4.1

# ── npm servers (exact versions + lockfile) ─────────────────────────────────
FROM ${NODE_IMAGE} AS npm-build
WORKDIR /opt/npm
COPY servers/npm/package.json servers/npm/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --omit=dev --no-audit --no-fund

# ── Python servers: one venv per server from its compiled lockfile ──────────
# Depends only on the catalog and lockfiles, so Rust changes don't reinstall Python.
FROM ${NODE_IMAGE} AS py-build
COPY --from=ghcr.io/astral-sh/uv:0.12.10 /uv /uvx /usr/local/bin/
ENV UV_PYTHON_INSTALL_DIR=/opt/python UV_NO_CACHE=1 UV_LINK_MODE=copy
RUN uv python install 3.13
COPY catalog/services.toml /tmp/services.toml
COPY servers/python /opt/py-locks
RUN set -eu; mkdir -p /opt/py-bin; \
    uv run --python 3.13 --no-project python -c 'import tomllib; [print(e["id"], e["command"]) for s in tomllib.load(open("/tmp/services.toml","rb"))["service"] for e in s.get("endpoint", []) if e["runtime"] == "pypi"]' | \
    while read -r id cmd; do \
      uv venv -q --python 3.13 "/opt/py/$id"; \
      uv pip install -q --python "/opt/py/$id/bin/python" -r "/opt/py-locks/$id.txt"; \
      ln -s "/opt/py/$id/bin/$cmd" "/opt/py-bin/$cmd"; \
    done

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM ${NODE_IMAGE} AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git \
 && rm -rf /var/lib/apt/lists/*
COPY --from=ghcr.io/astral-sh/uv:0.12.10 /uv /uvx /usr/local/bin/
ENV UV_PYTHON_INSTALL_DIR=/opt/python
COPY --from=py-build /opt/python /opt/python
COPY --from=py-build /opt/py /opt/py
COPY --from=py-build /opt/py-bin/ /opt/bin/
COPY --from=npm-build /opt/npm /opt/npm
COPY --from=go-build /out/ /opt/bin/
COPY --from=es-build /out/bin/ /opt/bin/
COPY --from=rust-build /out/ /opt/bin/
COPY catalog/services.toml /etc/tool-bench/services.toml

ENV PATH=/opt/bin:/opt/npm/node_modules/.bin:$PATH
WORKDIR /opt/tool-bench
COPY servers/snapshots /opt/tool-bench/snapshots
COPY servers/config /etc/tool-bench
EXPOSE 7101-7132
ENTRYPOINT ["tools-supervisor", "--ca-url", "http://world:9000/_bench/ca.pem"]
