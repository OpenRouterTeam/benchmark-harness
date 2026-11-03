# Agent container. Harbor installs the harness (Claude Code, Codex, ...) into it.
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git procps \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
