export interface AppTestScriptInput {
  readonly kind: "app";
  readonly description: readonly string[];
  readonly timeoutSec: number;
  readonly runArgs?: string;
  readonly cleanupLines?: readonly string[];
}

export interface AnswerTestScriptInput {
  readonly kind: "answer";
  readonly description: readonly string[];
  readonly answerFile: string;
}

export type AgentDxTestScriptInput = AppTestScriptInput | AnswerTestScriptInput;

const PROLOGUE = [
  "set -uo pipefail",
  "",
  "mkdir -p /logs/verifier",
  "echo 0 > /logs/verifier/reward.txt",
  "# Structured failure verdict; only the verifier fail() helper writes it.",
  "rm -f /logs/verifier/verdict.json",
  "cd /app",
  "",
] as const;

const VERIFY_HANDOFF = [
  "if node /tests/verify.ts > /logs/verifier/verify.log 2>&1; then",
  '  echo "SUBCHECK verified=pass"',
  "  echo 1 > /logs/verifier/reward.txt",
  "else",
  '  echo "SUBCHECK verified=fail"',
  "fi",
  "cat /logs/verifier/verify.log",
] as const;

export function renderAgentDxTestScript(input: AgentDxTestScriptInput): string {
  const header = [
    "#!/usr/bin/env bash",
    ...input.description.map((line) => `# ${line}`),
  ];
  const body = input.kind === "app" ? appBody(input) : answerBody(input);
  return [...header, ...PROLOGUE, ...body, ""].join("\n");
}

function appBody(input: AppTestScriptInput): readonly string[] {
  return [
    ...(input.cleanupLines === undefined
      ? []
      : [
          "cleanup() {",
          ...input.cleanupLines.map((line) => `  ${line}`),
          "}",
          "trap cleanup EXIT",
          "",
        ]),
    "if [ -f package.json ]; then",
    '  echo "SUBCHECK project_present=pass"',
    "else",
    '  echo "SUBCHECK project_present=fail"',
    "fi",
    "",
    "run_app() {",
    `  timeout "\${ADX_EVAL_TIMEOUT_SEC:-${input.timeoutSec}}" npm start${input.runArgs ?? ""} > /logs/verifier/run.log 2>&1`,
    "}",
    "",
    "# Transient runtime/network failures should not score a working build as 0;",
    "# the fresh run gets one retry.",
    "if ! run_app && ! run_app; then",
    '  echo "SUBCHECK app_ran=fail"',
    '  echo "npm start failed:"',
    "  # Prefix app output so it can never match verifier SUBCHECK/VERIFY FAIL lines.",
    "  sed 's/^/[app] /' /logs/verifier/run.log",
    "  exit 0",
    "fi",
    'echo "SUBCHECK app_ran=pass"',
    "",
    ...VERIFY_HANDOFF,
  ];
}

function answerBody(input: AnswerTestScriptInput): readonly string[] {
  return [
    `if [ -f ${input.answerFile} ]; then`,
    '  echo "SUBCHECK answer_present=pass"',
    "else",
    '  echo "SUBCHECK answer_present=fail"',
    `  echo "${input.answerFile} not found in /app"`,
    "  exit 0",
    "fi",
    "",
    ...VERIFY_HANDOFF,
  ];
}
