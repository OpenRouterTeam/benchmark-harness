#!/usr/bin/env bash
# Agent-DX verifier entry point. Runs the agent's submission fresh, then
# verifies the captured output against live OpenRouter generation records.
# Emits SUBCHECK diagnostics for failure categorization and writes 1 to
# /logs/verifier/reward.txt on success, 0 otherwise.
set -uo pipefail

mkdir -p /logs/verifier
echo 0 > /logs/verifier/reward.txt
# Structured failure verdict; only the verifier fail() helper writes it.
rm -f /logs/verifier/verdict.json
cd /app

if [ -f package.json ]; then
  echo "SUBCHECK project_present=pass"
else
  echo "SUBCHECK project_present=fail"
fi

run_app() {
  timeout "${ADX_EVAL_TIMEOUT_SEC:-180}" npm start > /logs/verifier/run.log 2>&1
}

# Transient runtime/network failures should not score a working build as 0;
# the fresh run gets one retry.
if ! run_app && ! run_app; then
  echo "SUBCHECK app_ran=fail"
  echo "npm start failed:"
  # Prefix app output so it can never match verifier SUBCHECK/VERIFY FAIL lines.
  sed 's/^/[app] /' /logs/verifier/run.log
  exit 0
fi
echo "SUBCHECK app_ran=pass"

if node /tests/verify.ts > /logs/verifier/verify.log 2>&1; then
  echo "SUBCHECK verified=pass"
  echo 1 > /logs/verifier/reward.txt
else
  echo "SUBCHECK verified=fail"
fi
cat /logs/verifier/verify.log
