#!/usr/bin/env bash
# Agent-DX verifier entry point for the question-style task. Checks the
# answer file exists, then verifies it covers the required BYOK ground truth.
# Emits SUBCHECK diagnostics and writes 1 to /logs/verifier/reward.txt on
# success.
set -uo pipefail

mkdir -p /logs/verifier
echo 0 > /logs/verifier/reward.txt
# Structured failure verdict; only the verifier fail() helper writes it.
rm -f /logs/verifier/verdict.json
cd /app

if [ -f ANSWER.md ]; then
  echo "SUBCHECK answer_present=pass"
else
  echo "SUBCHECK answer_present=fail"
  echo "ANSWER.md not found in /app"
  exit 0
fi

if node /tests/verify.ts > /logs/verifier/verify.log 2>&1; then
  echo "SUBCHECK verified=pass"
  echo 1 > /logs/verifier/reward.txt
else
  echo "SUBCHECK verified=fail"
fi
cat /logs/verifier/verify.log
