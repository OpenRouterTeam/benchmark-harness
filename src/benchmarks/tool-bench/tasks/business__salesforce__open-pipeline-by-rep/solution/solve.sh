#!/bin/sh
# Oracle: replays the scenario's tool calls through the real MCP endpoints in the tools container.
set -eu
DIR="$(cd "$(dirname "$0")" && pwd)"
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"tool-bench-oracle","version":"1"}}}'
call() {
  url="http://tools:$1/mcp"
  sid=$(curl -fsS -D - -o /dev/null -H 'content-type: application/json' -H 'accept: application/json, text/event-stream' -d "$INIT" "$url" | tr -d '\r' | awk -F': ' 'tolower($1)=="mcp-session-id" {print $2}')
  curl -fsS -o /dev/null -H 'content-type: application/json' -H "mcp-session-id: $sid" -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' "$url"
  curl -fsS -H 'content-type: application/json' -H 'accept: application/json, text/event-stream' -H "mcp-session-id: $sid" --data-binary @"$2" "$url"
  echo
}
call 7122 "$DIR/call-0.json"
call 7122 "$DIR/call-1.json"
cp "$DIR/answer.md" /app/answer.md
