#!/bin/bash
# deploy/smoke-ori.sh with a router arm: Claude Code's requests go through deploy/arm-proxy.ts,
# which records usage per request and, for `universal`, adds #54394's deferred_tools control.
#   ARM=off        ENABLE_TOOL_SEARCH=false (full payload), proxy -> $TARGET (default production)
#   ARM=native     Claude Code defaults (client ToolSearch), proxy -> $TARGET
#   ARM=universal  full payload plus the universal control, proxy -> $TARGET (default the local
#                  #54394 router on :8787, e.g. openrouter-web's real-CLI bench local-router.ts)
#   PROVIDER=anthropic pins every request to one provider, no fallbacks (same pin for every arm).
#   Claude Code runs on this host, so Bash is disabled by default: with it the model reads the
#   operator's real `gh` login and dotfiles. HOST_BASH=1 re-enables it (Harbor runs keep the stock
#   tool set inside a container).
#   deploy/smoke-arms.sh <scenario-id>...   MODEL=anthropic/claude-haiku-5.5 (default), results in $OUT
set -u
M="$(cd "$(dirname "$0")/.." && pwd)"
MODEL=${MODEL:-anthropic/claude-haiku-5.5}
ARM=${ARM:-native}
case "$ARM" in
  off|native) TARGET=${TARGET:-https://openrouter.ai}; UNIVERSAL="" ;;
  universal) TARGET=${TARGET:-http://127.0.0.1:8787}; UNIVERSAL="--universal" ;;
  *) echo "ARM must be off, native or universal" >&2; exit 2 ;;
esac
[ "$ARM" = native ] && SEARCH=true || SEARCH=false
OUT=${OUT:-/tmp/tool-bench-arms}; mkdir -p "$OUT"
# WORKER=n (0-8) runs beside other workers: own containers, network and ports.
W=${WORKER:-0}
PROXY_PORT=${PROXY_PORT:-$((18787 + W))}
ADMIN_PORT=$((19000 + W))
MCP_BASE=$((10000 + W * 1000))
[ "${HOST_BASH:-0}" = 1 ] && DISALLOWED="" || DISALLOWED="Bash BashOutput KillShell" 
NET=tb-smoke-$W; WORLD=tbs-world-$W; TOOLS=tbs-tools-$W
for id in "$@"; do
  slug=$(echo "$id" | tr '/' '_')_${ARM}${RUN:+_$RUN}
  docker rm -f $WORLD $TOOLS >/dev/null 2>&1; docker network rm $NET >/dev/null 2>&1
  docker network create $NET >/dev/null
  aliases=""; for h in $(cd $M && cargo run -q -p catalog --bin catalog -- aliases); do aliases="$aliases --network-alias $h"; done
  docker run -d --name $WORLD --network $NET --network-alias world $aliases -e SCENARIO_ID="$id" -p 127.0.0.1:$ADMIN_PORT:9000 ${WORLD_IMAGE:-tool-bench-world:dev} >/dev/null
  ports=""; for p in $(seq 7101 7132); do ports="$ports -p 127.0.0.1:$((MCP_BASE + p)):$p"; done
  docker run -d --name $TOOLS --network $NET --network-alias tools $ports tool-bench-tools:dev >/dev/null
  for i in $(seq 1 120); do curl -s -o /dev/null http://127.0.0.1:$((MCP_BASE + 7130))/mcp && break; sleep 1; done
  # MCP config with every endpoint, names as the catalog ids.
  { echo '{"mcpServers": {'; first=1
    for eid in $(cd $M && cargo run -q -p catalog --bin catalog -- endpoints | cut -f1); do
      port=$(cd $M && cargo run -q -p catalog --bin catalog -- port "$eid")
      [ $first = 1 ] || echo ','; first=0
      printf '"%s": {"type": "http", "url": "http://127.0.0.1:%s/mcp"}' "$eid" "$((MCP_BASE + port))"
    done; echo '}}'; } > $OUT/$slug.mcp.json
  work=$(mktemp -d $OUT/work.XXXX); cfg=$(mktemp -d $OUT/cfg.XXXX)
  # The host has no writable /app, so the answer file moves into the run's workdir and is posted
  # to the world before grading, as tests/test.sh does with /app/answer.md in a Harbor trial.
  instruction=$(sed "s|/app/answer.md|$work/answer.md|g" $M/scenarios/$id/instruction.md)
  : > $OUT/$slug.usage.jsonl
  bun $M/deploy/arm-proxy.ts --port $PROXY_PORT --target "$TARGET" --log $OUT/$slug.usage.jsonl $UNIVERSAL --model "$MODEL" ${PROVIDER:+--provider "$PROVIDER"} > $OUT/$slug.proxy.log 2>&1 &
  proxy_pid=$!
  for i in $(seq 1 30); do grep -q arm-proxy $OUT/$slug.proxy.log && break; sleep 0.5; done
  settings="{\"env\": {\"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:$PROXY_PORT/api\"}}"
  start=$(date +%s)
  (cd "$work" && env CLAUDE_CONFIG_DIR="$cfg" ENABLE_TOOL_SEARCH=$SEARCH timeout 1500 ori claude --model "$MODEL" -- \
      --settings "$settings" -p "$instruction" --output-format stream-json --verbose \
      ${DISALLOWED:+--disallowedTools $DISALLOWED} \
      --strict-mcp-config --mcp-config $OUT/$slug.mcp.json \
      --permission-mode bypassPermissions < /dev/null > $OUT/$slug.stream.jsonl 2> $OUT/$slug.stderr)
  echo "agent exit=$? seconds=$(( $(date +%s) - start )) arm=$ARM target=$TARGET model=$MODEL provider=${PROVIDER:--}" > $OUT/$slug.meta
  kill $proxy_pid 2>/dev/null; wait $proxy_pid 2>/dev/null
  overflow=0; grep -qiE 'prompt is too long|context_length_exceeded|maximum context length|exceeds the context window|input is too long' $OUT/$slug.stream.jsonl $OUT/$slug.stderr $OUT/$slug.usage.jsonl && overflow=1
  [ -f "$work/answer.md" ] && curl -s -X POST --data-binary @"$work/answer.md" http://127.0.0.1:$ADMIN_PORT/_bench/answer > /dev/null && cp "$work/answer.md" $OUT/$slug.answer.md
  curl -s http://127.0.0.1:$ADMIN_PORT/_bench/journal > $OUT/$slug.journal.json
  curl -s -X POST -H 'content-type: application/json' --data-binary @$M/scenarios/$id/../../../../tasks/$(echo "$id" | sed 's|/|__|g')/tests/expect.json "http://127.0.0.1:$ADMIN_PORT/_bench/grade?context_overflow=$overflow" > $OUT/$slug.reward.json
  # The grader's own context_overflow outcome needs an empty tools-container journal, which the MCP
  # servers' startup handshakes rule out; record what the harness saw so the tally can tell.
  tool_uses=$(grep -o '"type":"tool_use"' $OUT/$slug.stream.jsonl | wc -l | tr -d ' ')
  echo "overflow=$overflow tool_uses=$tool_uses" >> $OUT/$slug.meta
  echo "$id: $(cat $OUT/$slug.reward.json) $(tr '\n' ' ' < $OUT/$slug.meta)"
done
docker rm -f $WORLD $TOOLS >/dev/null 2>&1; docker network rm $NET >/dev/null 2>&1
