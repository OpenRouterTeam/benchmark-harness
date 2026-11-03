#!/bin/sh
# Verifies scenarios end to end with Harbor on local Docker:
#   oracle agent (replays solution/ over MCP) must score reward 1,
#   nop agent (does nothing) must score reward 0.
#
#   deploy/oracle.sh <scenario-id> [<scenario-id> ...]     e.g. code/github/file-login-bug
#   deploy/oracle.sh --all
#
# Builds the world image from this checkout as tool-bench-world:<branch> (so parallel
# worktrees don't collide) and uses the shared tool-bench-tools:dev image.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
TAG="$(git rev-parse --abbrev-ref HEAD | tr '/' '-')"
# Set WORLD_IMAGE to reuse a prebuilt world image (e.g. several oracle.sh runs in parallel).
PREBUILT_WORLD="${WORLD_IMAGE:-}"
WORLD_IMAGE="${WORLD_IMAGE:-tool-bench-world:$TAG}"
TOOLS_IMAGE="${TOOLS_IMAGE:-tool-bench-tools:dev}"
OUT="$(mktemp -d /tmp/tool-bench-oracle.XXXXXX)"
mkdir -p "$OUT/jobs"

[ -n "$PREBUILT_WORLD" ] || docker build -q -f images/world/Dockerfile -t "$WORLD_IMAGE" . >/dev/null
cargo run -q -p scenario --bin taskgen -- --out "$OUT/tasks" --world-image "$WORLD_IMAGE" --tools-image "$TOOLS_IMAGE" >/dev/null

if [ "${1:-}" = "--all" ]; then
  set -- $(ls "$OUT/tasks" | sed 's/__/\//g')
fi
status=0
for id in "$@"; do
  task="$OUT/tasks/$(echo "$id" | sed 's/\//__/g')"
  [ -d "$task" ] || { echo "$id: no such task"; status=1; continue; }
  for agent in oracle nop; do
    jobs="$OUT/jobs/$(basename "$task")-$agent"
    harbor run -q -p "$task" -a "$agent" -e docker -o "$jobs" >"$jobs.log" 2>&1 || true
    reward=$(find "$jobs" -path '*verifier/reward.json' 2>/dev/null | head -1 | xargs cat 2>/dev/null | tr -d ' \n' | sed -n 's/.*"reward":\([0-9.]*\).*/\1/p')
    want=$([ "$agent" = oracle ] && echo 1 || echo 0)
    if [ "${reward%.*}" = "$want" ]; then
      echo "$id: $agent reward=$reward ok"
    else
      echo "$id: $agent reward=${reward:-none} EXPECTED $want (logs: $jobs, $jobs.log)"
      status=1
    fi
  done
done
exit $status
