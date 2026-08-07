#!/usr/bin/env bash
# Agent-DX verifier entry point. Runs the agent's submission fresh, then
# verifies the preset exists with a valid model config and the
# inference ran through it. Emits SUBCHECK diagnostics and writes 1 to
# /logs/verifier/reward.txt on success.
set -uo pipefail

mkdir -p /logs/verifier
echo 0 > /logs/verifier/reward.txt
# Structured failure verdict; only the verifier fail() helper writes it.
rm -f /logs/verifier/verdict.json
cd /app

cleanup() {
  # Best-effort cleanup: the per-trial preset slug is account-global and would
  # otherwise accumulate on the benchmark account forever. Never affects reward.
  # Re-validate the slug at the shell boundary before interpolating it.
  case "${ADX_PRESET_SLUG:-}" in
    '' | *[!A-Za-z0-9_-]*) ;;
    *)
      curl -fsS -X DELETE \
        -H "Authorization: Bearer ${OPENROUTER_API_KEY}" \
        "${ADX_OPENROUTER_ORIGIN:-https://openrouter.ai}/api/v1/presets/${ADX_PRESET_SLUG}" \
        > /dev/null 2>&1 || true
      ;;
  esac
}
trap cleanup EXIT

if [ -f package.json ]; then
  echo "SUBCHECK project_present=pass"
else
  echo "SUBCHECK project_present=fail"
fi

run_app() {
  timeout "${ADX_EVAL_TIMEOUT_SEC:-240}" npm start > /logs/verifier/run.log 2>&1
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
