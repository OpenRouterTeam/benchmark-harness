#!/bin/sh
# Capture cloudflare/mcp@68c7ec0 (Code Mode) tools/list into /out/cloudflare.json.
# Run inside the tools image:  docker run --rm -v $PWD/servers:/servers -v /tmp/out:/out \
#   --entrypoint sh tool-bench-tools:dev /servers/capture/cloudflare/run.sh
set -eu
COMMIT=68c7ec0
git clone -q https://github.com/cloudflare/mcp /work && cd /work && git checkout -q "$COMMIT"
npm ci --ignore-scripts --no-audit --no-fund >/dev/null
npm install --no-save --no-audit --no-fund esbuild@0.25 >/dev/null
cp /servers/capture/cloudflare/entry.ts /servers/capture/cloudflare/stub-workers.mjs /servers/capture/cloudflare/build.mjs .
node build.mjs
toolmeter capture --name cloudflare --out /out -- node capture.mjs
