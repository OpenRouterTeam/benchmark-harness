#!/bin/sh
# Grades the trial: the world scores its own final state and journal against expect.json.
set -u
mkdir -p /logs/verifier
WORLD=http://world:9000
AUTH="authorization: Bearer $(cat /tests/admin-token)"
if [ -f /app/answer.md ]; then
  curl -fsS -H "$AUTH" -X POST --data-binary @/app/answer.md "$WORLD/_bench/answer" >/dev/null
fi
overflow=0
if grep -rqiE 'prompt is too long|context_length_exceeded|maximum context length|exceeds the context window|input is too long|too many total text bytes|request too large' /logs/agent 2>/dev/null; then
  overflow=1
fi
# Evidence for debugging and the determinism replay (world-replay <scenario> journal.json).
curl -fsS -H "$AUTH" "$WORLD/_bench/journal" -o /logs/verifier/journal.json || true
curl -fsS -H "$AUTH" "$WORLD/_bench/state" -o /logs/verifier/state.json || true
if ! curl -fsS -H "$AUTH" -X POST -H 'content-type: application/json' --data-binary @/tests/expect.json \
    "$WORLD/_bench/grade?context_overflow=$overflow" -o /logs/verifier/reward.json; then
  echo '{"reward": 0, "grader_error": 1}' > /logs/verifier/reward.json
fi
cat /logs/verifier/reward.json
