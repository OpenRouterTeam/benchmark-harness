#!/bin/sh
# Offline capture of every MCP endpoint exactly as an agent sees it:
#   world (mock APIs + TLS) <- tools (supervisor: bridges + replicas) <- toolmeter over HTTP.
# The network is --internal, so nothing can reach the internet.
#   deploy/capture-stack.sh [out-dir] [endpoint-id ...]   (default: servers/snapshots, all)
set -eu
OUT="${1:-servers/snapshots}"
case "$OUT" in /*) ;; *) OUT="$(cd "$(dirname "$0")/.." && pwd)/$OUT" ;; esac
[ $# -gt 0 ] && shift
ONLY="$*"
NET=tool-bench-capture
WORLD_IMAGE=${WORLD_IMAGE:-tool-bench-world:dev}
TOOLS_IMAGE=${TOOLS_IMAGE:-tool-bench-tools:dev}
cleanup() { docker rm -f tb-world tb-tools >/dev/null 2>&1 || true; docker network rm $NET >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup
docker network create --internal $NET >/dev/null

aliases=""
for host in $(docker run --rm --entrypoint catalog "$TOOLS_IMAGE" --file /etc/tool-bench/services.toml aliases); do
  aliases="$aliases --network-alias $host"
done
docker run -d --name tb-world --network $NET --network-alias world $aliases \
  -e SCENARIO_ID=example/github/basic "$WORLD_IMAGE" >/dev/null
docker run -d --name tb-tools --network $NET --network-alias tools "$TOOLS_IMAGE" >/dev/null

mkdir -p "$OUT"
docker run --rm --network $NET -e ONLY="$ONLY" -v "$OUT:/out" --entrypoint sh "$TOOLS_IMAGE" -c '
  set -u
  for i in $(seq 1 120); do curl -fs http://world:9000/_bench/health >/dev/null && break; sleep 0.5; done
  failures=0
  endpoints=$(catalog --file /etc/tool-bench/services.toml endpoints)
  while IFS="$(printf "\t")" read -r id runtime pkg ver; do
    if [ -n "$ONLY" ] && ! echo " $ONLY " | grep -q " $id "; then continue; fi
    port=$(catalog --file /etc/tool-bench/services.toml port "$id")
    for i in $(seq 1 60); do curl -s -o /dev/null http://tools:$port/mcp && break; sleep 0.5; done
    if ! toolmeter capture --name "$id" --out /out --http "http://tools:$port/mcp"; then
      echo "$id: FAILED"; failures=$((failures + 1))
    fi
  done <<EOF
$endpoints
EOF
  [ "$failures" = 0 ] || { echo "$failures endpoint(s) failed"; exit 1; }'
echo "--- tools container log (errors only)"
docker logs tb-tools 2>&1 | grep -iE "error|panic|failed" | head -40 || true
