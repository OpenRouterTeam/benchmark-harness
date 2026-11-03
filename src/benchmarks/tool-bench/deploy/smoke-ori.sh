#!/bin/bash
# Local model smoke run without Harbor: world + tools in Docker with ports published, Claude Code
# launched on the host through `ori claude` (uses your Ori/OpenRouter login) with ONLY the
# benchmark MCP servers (fresh CLAUDE_CONFIG_DIR, --strict-mcp-config), graded by /_bench/grade.
#   deploy/smoke-ori.sh <scenario-id>...            Claude Code defaults (tool search on)
#   MODE=full deploy/smoke-ori.sh <scenario-id>...  ENABLE_TOOL_SEARCH=false: full tool payload
#   MODEL=anthropic/claude-haiku-5.5 (default)      results in $OUT (default /tmp/tool-bench-smoke)
set -u
M="$(cd "$(dirname "$0")/.." && pwd)"
MODEL=${MODEL:-anthropic/claude-haiku-5.5}
OUT=${OUT:-/tmp/tool-bench-smoke}; mkdir -p "$OUT"
NET=tb-smoke
for id in "$@"; do
  slug=$(echo "$id" | tr '/' '_')${MODE:+_$MODE}
  docker rm -f tbs-world tbs-tools >/dev/null 2>&1; docker network rm $NET >/dev/null 2>&1
  docker network create $NET >/dev/null
  aliases=""; for h in $(cd $M && cargo run -q -p catalog --bin catalog -- aliases); do aliases="$aliases --network-alias $h"; done
  docker run -d --name tbs-world --network $NET --network-alias world $aliases -e SCENARIO_ID="$id" -p 127.0.0.1:19000:9000 ${WORLD_IMAGE:-tool-bench-world:dev} >/dev/null
  ports=""; for p in $(seq 7101 7132); do ports="$ports -p 127.0.0.1:1$p:$p"; done
  docker run -d --name tbs-tools --network $NET --network-alias tools $ports tool-bench-tools:dev >/dev/null
  for i in $(seq 1 120); do curl -s -o /dev/null http://127.0.0.1:17130/mcp && break; sleep 1; done
  # MCP config with every endpoint, names as the catalog ids.
  { echo '{"mcpServers": {'; first=1
    for eid in $(cd $M && cargo run -q -p catalog --bin catalog -- endpoints | cut -f1); do
      port=$(cd $M && cargo run -q -p catalog --bin catalog -- port "$eid")
      [ $first = 1 ] || echo ','; first=0
      printf '"%s": {"type": "http", "url": "http://127.0.0.1:1%s/mcp"}' "$eid" "$port"
    done; echo '}}'; } > $OUT/$slug.mcp.json
  work=$(mktemp -d $OUT/work.XXXX); cfg=$(mktemp -d $OUT/cfg.XXXX)
  instruction=$(sed "s|/app/answer.md|$work/answer.md|g" $M/scenarios/$id/instruction.md)
  start=$(date +%s)
  (cd "$work" && env CLAUDE_CONFIG_DIR="$cfg" ${MODE:+ENABLE_TOOL_SEARCH=false} timeout 1500 ori claude --model "$MODEL" -- \
      -p "$instruction" --output-format stream-json --verbose \
      --strict-mcp-config --mcp-config $OUT/$slug.mcp.json \
      --permission-mode bypassPermissions < /dev/null > $OUT/$slug.stream.jsonl 2> $OUT/$slug.stderr)
  echo "agent exit=$? seconds=$(( $(date +%s) - start ))" > $OUT/$slug.meta
  overflow=0; grep -qiE 'prompt is too long|context_length_exceeded|maximum context length|exceeds the context window|input is too long' $OUT/$slug.stream.jsonl $OUT/$slug.stderr && overflow=1
  [ -f "$work/answer.md" ] && curl -s -X POST --data-binary @"$work/answer.md" http://127.0.0.1:19000/_bench/answer > /dev/null && cp "$work/answer.md" $OUT/$slug.answer.md
  curl -s http://127.0.0.1:19000/_bench/journal > $OUT/$slug.journal.json
  curl -s -X POST -H 'content-type: application/json' --data-binary @$M/scenarios/$id/../../../../tasks/$(echo "$id" | sed 's|/|__|g')/tests/expect.json "http://127.0.0.1:19000/_bench/grade?context_overflow=$overflow" > $OUT/$slug.reward.json
  echo "$id: $(cat $OUT/$slug.reward.json) $(cat $OUT/$slug.meta)"
done
docker rm -f tbs-world tbs-tools >/dev/null 2>&1; docker network rm $NET >/dev/null 2>&1
