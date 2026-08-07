import type { ModelUsage } from "../../harness/core";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { AgentDxProfile, AgentDxSandboxKeyMode } from "./schema";
import {
  AGENT_DX_DOCS_ADDENDUM_PATH,
  AGENT_DX_DOCS_SNAPSHOT_PATH,
  AGENT_DX_DOCS_SOURCE_PATTERN,
  AGENT_DX_OPENCODE_PACKAGE_PATTERN,
  AGENT_DX_SKILLS_SOURCE_PATTERN,
  CLAUDE_CODE_PACKAGE,
  OPENROUTER_MCP_URL,
  REMOTE_AGENT_LOG,
  REMOTE_AGENT_STDERR_LOG,
} from "./schema";

export function usesSkills(profile: AgentDxProfile): boolean {
  return profile === "skills" || profile === "agents";
}

export function usesMcp(profile: AgentDxProfile): boolean {
  return profile === "mcp" || profile === "agents";
}

export function buildOpencodeImageSteps(input: {
  opencodePackage: string;
  profile: AgentDxProfile;
  skillsSource: string;
  docsSource: string;
}): string[] {
  if (!AGENT_DX_OPENCODE_PACKAGE_PATTERN.test(input.opencodePackage)) {
    throw new Error(
      `invalid opencodePackage "${input.opencodePackage}": must be an npm package name with optional @version`
    );
  }
  return [
    `RUN npm install -g ${input.opencodePackage}`,
    "RUN opencode --version",
    "RUN mkdir -p /app",
    ...(usesSkills(input.profile)
      ? buildSkillsInstallSteps(input.skillsSource)
      : []),
    ...(input.profile === "docs" ? [buildDocsFetchStep(input.docsSource)] : []),
  ];
}

export function buildSkillsInstallSteps(skillsSource: string): string[] {
  return [
    buildSkillsCloneStep(skillsSource),
    "RUN mkdir -p /root/.config/opencode/skills && cp -R /opt/openrouter-skills/skills/. /root/.config/opencode/skills/",
  ];
}

export function buildClaudeCodeImageSteps(input: {
  profile: AgentDxProfile;
  skillsSource: string;
  docsSource: string;
}): string[] {
  return [
    `RUN npm install -g ${CLAUDE_CODE_PACKAGE}`,
    "RUN claude --version",
    "RUN mkdir -p /app",
    ...(usesSkills(input.profile)
      ? [
          buildSkillsCloneStep(input.skillsSource),
          "RUN mkdir -p /root/.claude/skills && cp -R /opt/openrouter-skills/skills/. /root/.claude/skills/",
        ]
      : []),
    ...(input.profile === "docs" ? [buildDocsFetchStep(input.docsSource)] : []),
  ];
}

function buildDocsInstructions(docsAddendum: string | undefined): string {
  return [
    "# OpenRouter documentation",
    "",
    ...(docsAddendum === undefined
      ? []
      : [
          `Start with the latest OpenRouter guidance at ${AGENT_DX_DOCS_ADDENDUM_PATH}.`,
          "",
        ]),
    `A complete snapshot of the OpenRouter docs is available locally at ${AGENT_DX_DOCS_SNAPSHOT_PATH}.`,
    "Consult it (e.g. with grep) whenever you work with OpenRouter APIs, models, or features.",
  ].join("\n");
}

function keyEnvVarFor(sandboxKey: AgentDxSandboxKeyMode): string {
  return sandboxKey === "absent" ? "ADX_HARNESS_KEY" : "OPENROUTER_API_KEY";
}

function writeFileStep(contents: string, path: string): string {
  const b64 = Buffer.from(contents, "utf8").toString("base64");
  return `printf '%s' '${b64}' | base64 -d > ${path}`;
}

const RUN_SCRIPT_PROLOGUE: readonly string[] = [
  "set -uo pipefail",
  "mkdir -p /logs/agent",
  "cd /app",
];

function agentCommandLines(command: string): string[] {
  return [
    `${command} \\`,
    `  2>${REMOTE_AGENT_STDERR_LOG} </dev/null | tee ${REMOTE_AGENT_LOG}`,
  ];
}

function buildDocsWriteSteps(input: {
  profile: AgentDxProfile;
  docsAddendum: string | undefined;
  memoryFilePath: string;
}): string[] {
  if (input.profile !== "docs") {
    return [];
  }
  return [
    writeFileStep(
      buildDocsInstructions(input.docsAddendum),
      input.memoryFilePath
    ),
    ...(input.docsAddendum === undefined
      ? []
      : [writeFileStep(input.docsAddendum, AGENT_DX_DOCS_ADDENDUM_PATH)]),
  ];
}

export function buildOpencodeRunScript(input: {
  readonly profile: AgentDxProfile;
  readonly sandboxKey?: AgentDxSandboxKeyMode;
  readonly docsAddendum?: string;
  readonly mcpAddendum?: string;
}): string {
  const { profile, sandboxKey = "provided", docsAddendum, mcpAddendum } = input;
  const keyEnvVar = keyEnvVarFor(sandboxKey);
  const providerConfig = JSON.stringify({
    $schema: "https://opencode.ai/config.json",
    provider: { openrouter: { options: { apiKey: `{env:${keyEnvVar}}` } } },
  });
  const mcpConfig = JSON.stringify({
    $schema: "https://opencode.ai/config.json",
    ...(sandboxKey === "absent" && {
      provider: { openrouter: { options: { apiKey: `{env:${keyEnvVar}}` } } },
    }),
    mcp: {
      openrouter: {
        type: "remote",
        url: OPENROUTER_MCP_URL,
        headers: { Authorization: `Bearer {env:${keyEnvVar}}` },
      },
    },
  });
  return [
    ...RUN_SCRIPT_PROLOGUE,
    ...(sandboxKey === "absent"
      ? [
          "mkdir -p /root/.config/opencode",
          writeFileStep(providerConfig, "/root/.config/opencode/opencode.json"),
        ]
      : []),
    ...(usesMcp(profile)
      ? [writeFileStep(mcpConfig, "/app/opencode.json")]
      : []),
    ...(usesMcp(profile) && mcpAddendum !== undefined
      ? [writeFileStep(mcpAddendum, "/app/AGENTS.md")]
      : []),
    ...buildDocsWriteSteps({
      profile,
      docsAddendum,
      memoryFilePath: "/app/AGENTS.md",
    }),
    ...agentCommandLines(
      'opencode run --format json --auto -m "openrouter/$ADX_MODEL" "$(cat /instruction.md)"'
    ),
  ].join("\n");
}

export function buildClaudeCodeRunScript(input: {
  readonly profile: AgentDxProfile;
  readonly sandboxKey?: AgentDxSandboxKeyMode;
  readonly docsAddendum?: string;
  readonly mcpAddendum?: string;
}): string {
  const { profile, sandboxKey = "provided", docsAddendum, mcpAddendum } = input;
  const keyEnvVar = keyEnvVarFor(sandboxKey);
  const mcpConfig = JSON.stringify({
    mcpServers: {
      openrouter: {
        type: "http",
        url: OPENROUTER_MCP_URL,
        headers: { Authorization: `Bearer \${${keyEnvVar}}` },
      },
    },
  });
  const settings = JSON.stringify({ enableAllProjectMcpServers: true });
  return [
    ...RUN_SCRIPT_PROLOGUE,
    // eslint-disable-next-line no-template-curly-in-string -- shell param expansion in generated script
    'export ANTHROPIC_BASE_URL="${ADX_OPENROUTER_ORIGIN:-https://openrouter.ai}/api"',
    `export ANTHROPIC_AUTH_TOKEN="$${keyEnvVar}"`,
    'export ANTHROPIC_API_KEY=""',
    'export ANTHROPIC_MODEL="$ADX_MODEL"',
    "export IS_SANDBOX=1",
    ...(usesMcp(profile)
      ? [
          writeFileStep(mcpConfig, "/app/.mcp.json"),
          `mkdir -p /app/.claude && ${writeFileStep(settings, "/app/.claude/settings.json")}`,
        ]
      : []),
    ...(usesMcp(profile) && mcpAddendum !== undefined
      ? [writeFileStep(mcpAddendum, "/app/CLAUDE.md")]
      : []),
    ...buildDocsWriteSteps({
      profile,
      docsAddendum,
      memoryFilePath: "/app/CLAUDE.md",
    }),
    ...agentCommandLines(
      'claude -p --output-format stream-json --verbose --dangerously-skip-permissions "$(cat /instruction.md)"'
    ),
  ].join("\n");
}

export function parseOpencodeUsage(
  eventStream: string
): ModelUsage | undefined {
  let inputTokens = 0;
  let outputTokens = 0;
  let reasoningTokens = 0;
  let cacheTokens = 0;
  let totalCost = 0;
  let sawUsage = false;

  for (const line of eventStream.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "") {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(trimmed));
    if (
      Either.isLeft(parsed) ||
      !isRecord(parsed.right) ||
      parsed.right["type"] !== "step_finish"
    ) {
      continue;
    }
    const { part } = parsed.right;
    if (!isRecord(part)) {
      continue;
    }
    const { tokens } = part;
    if (isRecord(tokens)) {
      sawUsage = true;
      inputTokens += numberOrZero(tokens["input"]);
      outputTokens += numberOrZero(tokens["output"]);
      reasoningTokens += numberOrZero(tokens["reasoning"]);
      const { cache } = tokens;
      if (isRecord(cache)) {
        cacheTokens +=
          numberOrZero(cache["read"]) + numberOrZero(cache["write"]);
      }
    }
    totalCost += numberOrZero(part["cost"]);
  }

  if (!sawUsage) {
    return undefined;
  }
  return {
    inputTokens: inputTokens + cacheTokens,
    outputTokens,
    totalTokens: inputTokens + outputTokens + cacheTokens + reasoningTokens,
    reasoningTokens,
    totalCost,
  };
}

export function parseClaudeCodeUsage(
  eventStream: string
): ModelUsage | undefined {
  for (const line of eventStream.split("\n").toReversed()) {
    const trimmed = line.trim();
    if (trimmed === "") {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(trimmed));
    if (
      Either.isLeft(parsed) ||
      !isRecord(parsed.right) ||
      parsed.right["type"] !== "result"
    ) {
      continue;
    }
    const usage = parsed.right["usage"];
    if (!isRecord(usage)) {
      return undefined;
    }
    const inputTokens = numberOrZero(usage["input_tokens"]);
    const cacheTokens =
      numberOrZero(usage["cache_creation_input_tokens"]) +
      numberOrZero(usage["cache_read_input_tokens"]);
    const outputTokens = numberOrZero(usage["output_tokens"]);
    return {
      inputTokens: inputTokens + cacheTokens,
      outputTokens,
      totalTokens: inputTokens + cacheTokens + outputTokens,
      reasoningTokens: 0,
      totalCost: 0,
    };
  }
  return undefined;
}

function numberOrZero(value: unknown): number {
  return typeof value === "number" ? value : 0;
}

function buildDocsFetchStep(docsSource: string): string {
  if (!AGENT_DX_DOCS_SOURCE_PATTERN.test(docsSource)) {
    throw new Error(
      `invalid docsSource "${docsSource}": must be a plain https URL`
    );
  }
  return `RUN mkdir -p /opt/openrouter-docs && curl -fsSL ${docsSource} -o ${AGENT_DX_DOCS_SNAPSHOT_PATH}`;
}

function buildSkillsCloneStep(skillsSource: string): string {
  if (!AGENT_DX_SKILLS_SOURCE_PATTERN.test(skillsSource)) {
    throw new Error(
      `invalid skillsSource "${skillsSource}": must be an https git URL with optional #ref`
    );
  }
  const hashIndex = skillsSource.lastIndexOf("#");
  const url =
    hashIndex === -1 ? skillsSource : skillsSource.slice(0, hashIndex);
  const ref = hashIndex === -1 ? "" : skillsSource.slice(hashIndex + 1);
  return ref === ""
    ? `RUN git clone --depth 1 ${url} /opt/openrouter-skills`
    : `RUN git clone ${url} /opt/openrouter-skills && git -C /opt/openrouter-skills checkout ${ref}`;
}
