import type { AgentDxTestScriptInput } from "./test-script";

export const AGENT_DX_TEST_SCRIPTS: Readonly<
  Record<string, AgentDxTestScriptInput>
> = {
  "agent-sdk-workflow": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the composite pipeline end to end. Emits SUBCHECK",
      "diagnostics and writes 1 to /logs/verifier/reward.txt on success.",
    ],
    timeoutSec: 420,
  },
  "basic-completion": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "byok-config": {
    kind: "answer",
    description: [
      "Agent-DX verifier entry point for the question-style task. Checks the",
      "answer file exists, then verifies it covers the required BYOK ground truth.",
      "Emits SUBCHECK diagnostics and writes 1 to /logs/verifier/reward.txt on",
      "success.",
    ],
    answerFile: "ANSWER.md",
  },
  "compiled-esm-scaffold": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "fallback-resilience": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "image-input": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
    runArgs: " -- /tests/fixture.png",
  },
  "llm-feature-add": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the summarize feature works. Outcome-based and provider-agnostic:",
      "the discoverability signal comes from the workspace evidence scan, not from",
      "this verifier. Emits SUBCHECK diagnostics and writes 1 to",
      "/logs/verifier/reward.txt on success.",
    ],
    timeoutSec: 180,
  },
  "model-discovery": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "preset-config": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the preset exists with a valid model config and the",
      "inference ran through it. Emits SUBCHECK diagnostics and writes 1 to",
      "/logs/verifier/reward.txt on success.",
    ],
    timeoutSec: 240,
    cleanupLines: [
      "# Best-effort cleanup: the per-trial preset slug is account-global and would",
      "# otherwise accumulate on the benchmark account forever. Never affects reward.",
      "# Re-validate the slug at the shell boundary before interpolating it.",
      // eslint-disable-next-line no-template-curly-in-string -- shell param expansion in generated script
      'case "${ADX_PRESET_SLUG:-}" in',
      "  '' | *[!A-Za-z0-9_-]*) ;;",
      "  *)",
      "    curl -fsS -X DELETE \\",
      // eslint-disable-next-line no-template-curly-in-string -- shell param expansion in generated script
      '      -H "Authorization: Bearer ${OPENROUTER_API_KEY}" \\',
      // eslint-disable-next-line no-template-curly-in-string -- shell param expansion in generated script
      '      "${ADX_OPENROUTER_ORIGIN:-https://openrouter.ai}/api/v1/presets/${ADX_PRESET_SLUG}" \\',
      "      > /dev/null 2>&1 || true",
      "    ;;",
      "esac",
    ],
  },
  "provider-pinning": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the pinned request was actually served by the pinned provider and",
      "the bad-provider request failed closed. Emits SUBCHECK diagnostics and",
      "writes 1 to /logs/verifier/reward.txt on success.",
    ],
    timeoutSec: 180,
  },
  "streaming-usage": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "structured-outputs": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "tool-calling-loop": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "web-search": {
    kind: "app",
    description: [
      "Agent-DX verifier entry point. Runs the agent's submission fresh, then",
      "verifies the captured output against live OpenRouter generation records.",
      "Emits SUBCHECK diagnostics for failure categorization and writes 1 to",
      "/logs/verifier/reward.txt on success, 0 otherwise.",
    ],
    timeoutSec: 180,
  },
  "which-model-question": {
    kind: "answer",
    description: [
      "Agent-DX verifier entry point for the question-style discoverability task.",
      "Checks the answer file exists, then verifies its internal consistency.",
      "Emits SUBCHECK diagnostics and writes 1 to /logs/verifier/reward.txt on",
      "success.",
    ],
    answerFile: "ANSWER.md",
  },
};
