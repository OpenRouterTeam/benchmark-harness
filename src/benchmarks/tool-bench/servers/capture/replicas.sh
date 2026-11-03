#!/bin/sh
# Capture the tool lists served by replica endpoints from their pinned source servers.
# Run inside the tools image (has node, uv, toolmeter):
#   docker run --rm -v $PWD/servers:/servers -v /tmp/out:/out --entrypoint sh \
#     tool-bench-tools:dev /servers/capture/replicas.sh [vercel|zendesk|mongodb|google-workspace|linear]
set -u
OUT=/out
only="${1:-all}"
run() { [ "$only" = all ] || [ "$only" = "$1" ]; }
status=0

if run vercel; then
  git clone -q https://github.com/nganiet/mcp-vercel /work/vercel && cd /work/vercel && git checkout -q e6f9173 \
    && npm ci --no-audit --no-fund >/dev/null && npm run -s build >/dev/null \
    && toolmeter capture --name vercel --out $OUT --env VERCEL_API_TOKEN=vercel_toolbench -- node build/index.js || status=1
fi
if run zendesk; then
  # The project's own uv.lock pins mcp 1.1.2; newer mcp releases break its imports.
  git clone -q https://github.com/reminia/zendesk-mcp-server /work/zendesk && cd /work/zendesk && git checkout -q 806d1ca \
    && uv sync --frozen --python 3.13 -q \
    && toolmeter capture --name zendesk --out $OUT \
      --env ZENDESK_SUBDOMAIN=octo-corp --env ZENDESK_EMAIL=agent@octo-corp.example --env ZENDESK_API_TOKEN=zd-toolbench \
      -- /work/zendesk/.venv/bin/zendesk || status=1
fi
if run mongodb; then
  toolmeter capture --name mongodb --out $OUT \
    --env MDB_MCP_API_CLIENT_ID=mdb_sa_id_toolbench --env MDB_MCP_API_CLIENT_SECRET=mdb_sa_sk_toolbench \
    --env MDB_MCP_CONNECTION_STRING=mongodb://world:27017/octo --env MDB_MCP_TELEMETRY=disabled \
    -- npx -y mongodb-mcp-server@3.2.2 || status=1
fi
if run google-workspace; then
  toolmeter capture --name google-workspace --out $OUT \
    --env GOOGLE_OAUTH_CLIENT_ID=toolbench.apps.googleusercontent.com --env GOOGLE_OAUTH_CLIENT_SECRET=toolbench \
    --env USER_GOOGLE_EMAIL=ops@octo-corp.example \
    -- uv tool run --python 3.13 workspace-mcp==2.2.1 --tool-tier complete || status=1
fi
if run linear; then
  toolmeter capture --name linear --out $OUT --env LINEAR_API_TOKEN=lin_api_toolbench \
    -- npx -y @tacticlaunch/mcp-linear@1.4.5 || status=1
fi
exit $status
