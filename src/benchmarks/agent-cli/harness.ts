import type {
  ModelMessage,
  ModelUsage,
  ResponseItem,
} from "../../harness/core";
import { MessageRole } from "../../harness/core";
import { Either } from "../../internal/either";
import { definedValues, isRecord } from "../../internal/guards";
import {
  buildGenerationProxyPrelude,
  GENERATION_PROXY_BASE_URL,
  GENERATION_PROXY_BASE_URL_ENV,
  parseProxyGenerationIds,
} from "./generation-proxy";
import type { OriAgent, OriChannel, OriReasoningEffort } from "./schema";
import { assertValidAgentPackage, DEFAULT_CLAUDE_PACKAGE } from "./schema";

const NODE_VERSION = "22" as const;

export const NVM_INSTALL_URL =
  "https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.2/install.sh" as const;

export const NVM_INSTALL_SHA256 =
  "a909fdd01765379ebc5983674adafb8bc9de6d928bfa188761309d4a0c36be0f" as const;

export const ORI_INSTALL_DIR = "/usr/local/bin" as const;

export const DEFAULT_PI_AGENT_PACKAGE =
  "@earendil-works/pi-coding-agent@0.84.2" as const;

export const DEFAULT_PRIME_AGENT_PACKAGE =
  "https://pub-728493de92a943e2a9b2d17b4719f318.r2.dev/releases/v0.9.1/prime-agent-0.9.1.tgz" as const;

export const DEFAULT_OMP_PACKAGE = "@oh-my-pi/pi-coding-agent@18.1.2" as const;

export const DEFAULT_CODEX_PACKAGE = "@openai/codex@0.153.1" as const;

export const DEFAULT_OPENCODE_PACKAGE = "opencode-ai@1.18.27" as const;

export const DEFAULT_KILO_PACKAGE = "@kilocode/cli@7.5.9" as const;

export const OMP_BUN_VERSION = "bun-v1.3.14" as const;

export const BUN_RELEASE_URL =
  `https://github.com/oven-sh/bun/releases/download/${OMP_BUN_VERSION}/bun-linux-x64.zip` as const;

export const BUN_RELEASE_SHA256 =
  "951ee2aee855f08595aeec6225226a298d3fea83a3dcd6465c09cbccdf7e848f" as const;

function verifiedDownload(url: string, sha256: string, dest: string): string {
  return `curl -fsSL ${JSON.stringify(url)} -o ${dest} && echo "${sha256}  ${dest}" | sha256sum -c -`;
}

export const DEFAULT_AGENT_RUNTIME_URL =
  "https://github.com/OpenRouterTeam/benchmark-harness/releases/download/agent-runtime-v2/agent-runtime-linux-x64-v2.tar.zst" as const;

export const DEFAULT_AGENT_RUNTIME_SHA256 =
  "7b59b9f3053b729f3d32da06154f68552e753b77da0b1dc497f53a860a8c8af2" as const;

export interface OriRunScriptOptions {
  readonly instructionPath: string;
  readonly logPath: string;
  readonly reasoningEffort: OriReasoningEffort;
  readonly hasSystemPrompt: boolean;
  readonly hasAppendSystemPrompt: boolean;
  readonly hasAllowedTools: boolean;
  readonly hasDisallowedTools: boolean;
  readonly isolateAgentConfig: boolean;
}

export interface OriImageStepsOptions {
  readonly agentPackage: string;
}

export interface OriBootstrapOptions {
  readonly oriInstallUrl: string;
  readonly oriChannel: OriChannel;
}

export interface OriAgentRun {
  readonly usage: ModelUsage | undefined;
  readonly generationIds: readonly string[];
  readonly generationTimeMs: number | undefined;
  readonly finalText: string | undefined;
  readonly assistantMessages: readonly ModelMessage[];
  readonly responseItems: readonly ResponseItem[];
  readonly isError: boolean;
  readonly apiErrorStatus: string | undefined;
  readonly turns: number | undefined;
  readonly toolCalls: number;
}

export interface OriHarnessDef {
  readonly id: OriAgent;
  readonly defaultPackage: string;
  readonly binaryName: string;
  readonly remoteLogPath: string;
  readonly imageBuildSteps: (options: OriImageStepsOptions) => string[];
  readonly buildBootstrapScript: (options: OriBootstrapOptions) => string;
  readonly buildRunScript: (options: OriRunScriptOptions) => string;
  readonly parseRun: (stdout: string) => OriAgentRun;
}

function buildImageSteps(opts: {
  agentPackage: string;
  binaryName: string;
  installCommand?: string;
}): string[] {
  assertValidAgentPackage(opts.agentPackage);
  const installCommand =
    opts.installCommand ??
    `npm install -g ${JSON.stringify(opts.agentPackage)}`;
  const nvmScript = "/tmp/nvm-install.sh";
  return [
    "RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates git",
    "ENV NVM_DIR=/root/.nvm",
    `RUN ${verifiedDownload(NVM_INSTALL_URL, NVM_INSTALL_SHA256, nvmScript)} && bash ${nvmScript} && rm -f ${nvmScript}`,
    `RUN . /root/.nvm/nvm.sh && nvm install ${NODE_VERSION} && ${installCommand} && ln -sf $(which ${opts.binaryName}) /usr/local/bin/${opts.binaryName} && ln -sf $(which node) /usr/local/bin/node && ln -sf $(which npm) /usr/local/bin/npm`,
    `RUN ${opts.binaryName} --version`,
  ];
}

function buildAgentImageSteps(opts: {
  agentPackage: string;
  binaryName: string;
  defaultPackage: string;
  installCommand?: string;
}): string[] {
  if (opts.agentPackage === opts.defaultPackage) {
    const archivePath = "/tmp/agent-runtime.tar.zst";
    return [
      "RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates git zstd",
      `RUN ${verifiedDownload(DEFAULT_AGENT_RUNTIME_URL, DEFAULT_AGENT_RUNTIME_SHA256, archivePath)} && zstd -dc ${archivePath} | tar -x -C / && rm -f ${archivePath}`,
      'ENV PATH="/root/.local/bin:$PATH"',
      "RUN ln -sf /opt/agent-runtime/app/node_modules/.bin/claude /usr/local/bin/claude && ln -sf /opt/agent-runtime/app/node_modules/.bin/pi /usr/local/bin/pi && ln -sf /opt/agent-runtime/app/node_modules/.bin/prime-agent /usr/local/bin/prime-agent && ln -sf /opt/agent-runtime/node/bin/node /usr/local/bin/node && ln -sf /opt/agent-runtime/node/bin/npm /usr/local/bin/npm && ln -sf /opt/agent-runtime/node/bin/npx /usr/local/bin/npx",
      `RUN ${opts.binaryName} --version`,
    ];
  }
  return buildImageSteps(
    definedValues({
      agentPackage: opts.agentPackage,
      binaryName: opts.binaryName,
      installCommand: opts.installCommand,
    })
  );
}

function buildOmpImageSteps(agentPackage: string): string[] {
  assertValidAgentPackage(agentPackage);
  const bunZip = "/tmp/bun.zip";
  return [
    "RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates git unzip",
    "ENV BUN_INSTALL=/root/.bun",
    `RUN ${verifiedDownload(BUN_RELEASE_URL, BUN_RELEASE_SHA256, bunZip)} && unzip -q ${bunZip} -d /tmp && install -m 0755 /tmp/bun-linux-x64/bun /usr/local/bin/bun && rm -rf ${bunZip} /tmp/bun-linux-x64`,
    `RUN bun install -g ${JSON.stringify(agentPackage)} && ln -sf /root/.bun/bin/omp /usr/local/bin/omp`,
    "RUN omp --version",
  ];
}

function buildBootstrapScript(opts: {
  oriInstallUrl: string;
  oriChannel: OriChannel;
  binaryName: string;
}): string {
  const channelPrefix =
    opts.oriChannel === "stable" ? "" : `ORI_CHANNEL=${opts.oriChannel} `;
  return [
    "set -euo pipefail",
    `curl -fsSL ${opts.oriInstallUrl} | ${channelPrefix}ORI_INSTALL_DIR=${ORI_INSTALL_DIR} bash`,
    `ori --version && ${opts.binaryName} --version`,
  ].join("\n");
}

function numberField(source: unknown, key: string): number {
  if (!isRecord(source)) {
    return 0;
  }
  const raw = source[key];
  return typeof raw === "number" ? raw : 0;
}

function optionalNumberField(source: unknown, key: string): number | undefined {
  if (!isRecord(source)) {
    return undefined;
  }
  const raw = source[key];
  return typeof raw === "number" ? raw : undefined;
}

function optionalStringField(source: unknown, key: string): string | undefined {
  if (!isRecord(source)) {
    return undefined;
  }
  const raw = source[key];
  return typeof raw === "string" ? raw : undefined;
}

function textFromContent(content: unknown): string {
  if (typeof content === "string") {
    return content;
  }
  if (!Array.isArray(content)) {
    return "";
  }
  const parts: string[] = [];
  for (const block of content) {
    if (!isRecord(block)) {
      continue;
    }
    const text = block["text"];
    if (block["type"] === "text" && typeof text === "string") {
      parts.push(text);
    }
  }
  return parts.join("");
}

function reasoningFromContent(content: unknown): string | undefined {
  if (!Array.isArray(content)) {
    return undefined;
  }
  const parts: string[] = [];
  for (const block of content) {
    if (!isRecord(block)) {
      continue;
    }
    const thinking = block["thinking"];
    if (block["type"] === "thinking" && typeof thinking === "string") {
      parts.push(thinking);
    }
  }
  return parts.length > 0 ? parts.join("\n") : undefined;
}

function usageFromResult(result: Record<string, unknown>): ModelUsage {
  const { usage } = result;
  const inputTokens =
    numberField(usage, "input_tokens") +
    numberField(usage, "cache_creation_input_tokens") +
    numberField(usage, "cache_read_input_tokens");
  const outputTokens = numberField(usage, "output_tokens");
  const details = isRecord(usage) ? usage["output_tokens_details"] : undefined;
  const reasoningTokens = numberField(details, "thinking_tokens");
  const serverToolUse = isRecord(usage) ? usage["server_tool_use"] : undefined;
  const webSearchRequests = optionalNumberField(
    serverToolUse,
    "web_search_requests"
  );
  return {
    inputTokens,
    outputTokens,
    totalTokens: inputTokens + outputTokens,
    reasoningTokens,
    totalCost: numberField(result, "total_cost_usd"),
    ...definedValues({
      serverToolUse:
        webSearchRequests !== undefined ? { webSearchRequests } : undefined,
    }),
  };
}

function parseClaudeStream(stdout: string): OriAgentRun {
  const generationIds: string[] = [];
  const assistantMessages: ModelMessage[] = [];
  const responseItems: ResponseItem[] = [];
  let usage: ModelUsage | undefined;
  let generationTimeMs: number | undefined;
  let finalText: string | undefined;
  let isError = false;
  let apiErrorStatus: string | undefined;
  let turns: number | undefined;
  let toolCalls = 0;
  for (const line of stdout.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(trimmed));
    if (Either.isLeft(parsed) || !isRecord(parsed.right)) {
      continue;
    }
    const event = parsed.right;
    responseItems.push(event);
    const eventType = event["type"];
    if (eventType === "assistant") {
      const { message } = event;
      if (!isRecord(message)) {
        continue;
      }
      const id = message["id"];
      if (
        typeof id === "string" &&
        id.length > 0 &&
        !generationIds.includes(id)
      ) {
        generationIds.push(id);
      }
      toolCalls += countToolUseBlocks(message["content"]);
      const content = textFromContent(message["content"]);
      const reasoning = reasoningFromContent(message["content"]);
      const model = optionalStringField(message, "model");
      if (content.length > 0 || reasoning !== undefined) {
        assistantMessages.push(
          definedValues({
            role: MessageRole.Assistant,
            content,
            reasoning,
            model,
          })
        );
      }
      continue;
    }
    if (eventType === "result") {
      usage = usageFromResult(event);
      generationTimeMs = optionalNumberField(event, "duration_ms");
      finalText = optionalStringField(event, "result");
      isError = event["is_error"] === true;
      apiErrorStatus = optionalStringField(event, "api_error_status");
      turns = optionalNumberField(event, "num_turns");
    }
  }
  return {
    usage,
    generationIds,
    generationTimeMs,
    finalText,
    assistantMessages,
    responseItems,
    isError,
    apiErrorStatus,
    turns,
    toolCalls,
  };
}

function countToolUseBlocks(content: unknown): number {
  if (!Array.isArray(content)) {
    return 0;
  }
  let count = 0;
  for (const block of content) {
    if (isRecord(block) && block["type"] === "tool_use") {
      count++;
    }
  }
  return count;
}

const CLAUDE_HARNESS: OriHarnessDef = {
  id: "claude",
  defaultPackage: DEFAULT_CLAUDE_PACKAGE,
  binaryName: "claude",
  remoteLogPath: "/logs/agent/claude.txt",
  imageBuildSteps: (options) =>
    buildAgentImageSteps({
      ...options,
      binaryName: "claude",
      defaultPackage: DEFAULT_CLAUDE_PACKAGE,
    }),
  buildBootstrapScript: (options) =>
    buildBootstrapScript({ ...options, binaryName: "claude" }),
  buildRunScript: (options) =>
    [
      "set -euo pipefail",
      "export HOME=/root",
      "export IS_SANDBOX=1",
      "mkdir -p /logs/agent",
      'ori claude --model "$TB_MODEL" \\',
      `  --reasoning-effort ${options.reasoningEffort} -- \\`,
      `  -p "$(cat ${options.instructionPath})" \\`,
      "  --output-format stream-json \\",
      "  --verbose \\",
      "  --permission-mode bypassPermissions \\",
      ...(options.hasSystemPrompt
        ? ['  --system-prompt "$TB_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAppendSystemPrompt
        ? ['  --append-system-prompt "$TB_APPEND_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAllowedTools
        ? ['  --allowedTools "$TB_ALLOWED_TOOLS" \\']
        : []),
      ...(options.hasDisallowedTools
        ? ['  --disallowedTools "$TB_DISALLOWED_TOOLS" \\']
        : []),
      ...(options.isolateAgentConfig
        ? ["  --exclude-dynamic-system-prompt-sections \\"]
        : []),
      `  2>&1 </dev/null | stdbuf -oL tee ${options.logPath}`,
    ].join("\n"),
  parseRun: parseClaudeStream,
};

const ORI_PI_HARNESS: OriHarnessDef = {
  id: "pi",
  defaultPackage: DEFAULT_PI_AGENT_PACKAGE,
  binaryName: "pi",
  remoteLogPath: "/logs/agent/pi.txt",
  imageBuildSteps: (options) =>
    buildAgentImageSteps({
      ...options,
      binaryName: "pi",
      defaultPackage: DEFAULT_PI_AGENT_PACKAGE,
    }),
  buildBootstrapScript: (options) =>
    buildBootstrapScript({ ...options, binaryName: "pi" }),
  buildRunScript: (options) =>
    [
      "set -euo pipefail",
      "export HOME=/root",
      "mkdir -p /logs/agent",
      'ori pi --model "$TB_MODEL" \\',
      `  --reasoning-effort ${options.reasoningEffort} -- \\`,
      "  --print --mode json --no-session \\",
      ...(options.hasSystemPrompt
        ? ['  --system-prompt "$TB_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAppendSystemPrompt
        ? ['  --append-system-prompt "$TB_APPEND_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAllowedTools ? ['  --tools "$TB_ALLOWED_TOOLS" \\'] : []),
      ...(options.hasDisallowedTools
        ? ['  --exclude-tools "$TB_DISALLOWED_TOOLS" \\']
        : []),
      ...(options.isolateAgentConfig
        ? [
            "  --no-extensions \\",
            "  --no-skills \\",
            "  --no-prompt-templates \\",
            "  --no-context-files \\",
          ]
        : []),
      `  "$(cat ${options.instructionPath})" \\`,
      `  2>&1 </dev/null | grep -v '"type":"message_update"' | stdbuf -oL tee ${options.logPath}`,
    ].join("\n"),
  parseRun: parseJsonAgentStream,
};

const PRIME_AGENT_HARNESS: OriHarnessDef = {
  id: "prime-agent",
  defaultPackage: DEFAULT_PRIME_AGENT_PACKAGE,
  binaryName: "prime-agent",
  remoteLogPath: "/logs/agent/prime-agent.txt",
  imageBuildSteps: (options) =>
    buildAgentImageSteps({
      ...options,
      binaryName: "prime-agent",
      defaultPackage: DEFAULT_PRIME_AGENT_PACKAGE,
      installCommand: [
        "PRIME_AGENT_BOOTSTRAP_TOOLS_ON_INSTALL=1",
        "PRIME_AGENT_BOOTSTRAP_KERNEL_ON_INSTALL=1",
        "PRIME_AGENT_INSTALL_UV=1",
        "npm install -g --no-fund --no-audit --loglevel=error --progress=false",
        JSON.stringify(options.agentPackage),
      ].join(" "),
    }),
  buildBootstrapScript: (options) =>
    buildBootstrapScript({ ...options, binaryName: "prime-agent" }),
  buildRunScript: (options) =>
    [
      "set -euo pipefail",
      "export HOME=/root",
      "mkdir -p /logs/agent",
      ...(options.hasDisallowedTools
        ? ['echo "Prime Agent does not support disallowedTools" >&2', "exit 2"]
        : []),
      'ori prime-agent --model "$TB_MODEL" \\',
      `  --reasoning-effort ${options.reasoningEffort} -- \\`,
      "  --print --mode json --no-session \\",
      ...(options.hasSystemPrompt
        ? ['  --system-prompt "$TB_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAppendSystemPrompt
        ? ['  --append-system-prompt "$TB_APPEND_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAllowedTools
        ? [`  --tools "\${TB_ALLOWED_TOOLS// /,}" \\`]
        : []),
      ...(options.isolateAgentConfig
        ? [
            "  --no-extensions \\",
            "  --no-skills \\",
            "  --no-prompt-templates \\",
            "  --no-themes \\",
            "  --no-context-files \\",
          ]
        : []),
      "  -- \\",
      `  "$(cat ${options.instructionPath})" \\`,
      `  2>&1 </dev/null | grep -v '"type":"message_update"' | stdbuf -oL tee ${options.logPath}`,
    ].join("\n"),
  parseRun: parseJsonAgentStream,
};

const OMP_HARNESS: OriHarnessDef = {
  id: "omp",
  defaultPackage: DEFAULT_OMP_PACKAGE,
  binaryName: "omp",
  remoteLogPath: "/logs/agent/omp.txt",
  imageBuildSteps: (options) => buildOmpImageSteps(options.agentPackage),
  buildBootstrapScript: (options) =>
    buildBootstrapScript({ ...options, binaryName: "omp" }),
  buildRunScript: (options) =>
    [
      "set -euo pipefail",
      "export HOME=/root",
      "mkdir -p /logs/agent",
      ...(options.hasDisallowedTools
        ? ['echo "omp does not support disallowedTools" >&2', "exit 2"]
        : []),
      'ori omp --model "$TB_MODEL" \\',
      `  --reasoning-effort ${options.reasoningEffort} -- \\`,
      "  --print --mode json --no-session --yolo \\",
      ...(options.hasSystemPrompt
        ? ['  --system-prompt "$TB_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAppendSystemPrompt
        ? ['  --append-system-prompt "$TB_APPEND_SYSTEM_PROMPT" \\']
        : []),
      ...(options.hasAllowedTools
        ? [`  --tools "\${TB_ALLOWED_TOOLS// /,}" \\`]
        : []),
      ...(options.isolateAgentConfig
        ? ["  --no-extensions \\", "  --no-skills \\", "  --no-rules \\"]
        : []),
      `  "$(cat ${options.instructionPath})" \\`,
      `  2>&1 </dev/null | grep -v '"type":"message_update"' | stdbuf -oL tee ${options.logPath}`,
    ].join("\n"),
  parseRun: parseJsonAgentStream,
};

const CODEX_SYSTEM_PROMPT_PATH = "/tmp/agent-system-prompt.md" as const;
const OPENCODE_APPEND_PROMPT_PATH = "/tmp/agent-append-prompt.md" as const;

const CODEX_HARNESS: OriHarnessDef = {
  id: "codex",
  defaultPackage: DEFAULT_CODEX_PACKAGE,
  binaryName: "codex",
  remoteLogPath: "/logs/agent/codex.txt",
  imageBuildSteps: (options) =>
    buildImageSteps({ ...options, binaryName: "codex" }),
  buildBootstrapScript: (options) =>
    buildBootstrapScript({ ...options, binaryName: "codex" }),
  buildRunScript: (options) =>
    [
      "set -euo pipefail",
      "export HOME=/root",
      "export RUST_LOG=error",
      "mkdir -p /logs/agent",
      ...(options.hasAllowedTools || options.hasDisallowedTools
        ? [
            'echo "Codex does not support allowedTools or disallowedTools" >&2',
            "exit 2",
          ]
        : []),
      ...(options.hasSystemPrompt
        ? [`printf '%s' "$TB_SYSTEM_PROMPT" > ${CODEX_SYSTEM_PROMPT_PATH}`]
        : []),
      ...buildGenerationProxyPrelude(options.logPath),
      'ori codex --model "$TB_MODEL" \\',
      `  --reasoning-effort ${options.reasoningEffort} -- \\`,
      "  exec --json --ephemeral --skip-git-repo-check \\",
      "  --dangerously-bypass-approvals-and-sandbox \\",
      '  -m "$TB_MODEL" \\',
      `  -c model_reasoning_effort=${options.reasoningEffort} \\`,
      "  -c model_provider=openrouter \\",
      "  -c \"model_providers.openrouter.name='OpenRouter'\" \\",
      `  -c "model_providers.openrouter.base_url='${GENERATION_PROXY_BASE_URL}'" \\`,
      "  -c \"model_providers.openrouter.wire_api='responses'\" \\",
      "  -c \"model_providers.openrouter.env_http_headers.X-Session-Id='ORI_OPENROUTER_SESSION_ID'\" \\",
      "  -c \"model_providers.openrouter.auth.command='sh'\" \\",
      "  -c \"model_providers.openrouter.auth.args=['-c','echo \\$OPENROUTER_API_KEY']\" \\",
      ...(options.hasSystemPrompt
        ? [`  -c "model_instructions_file='${CODEX_SYSTEM_PROMPT_PATH}'" \\`]
        : []),
      ...(options.hasAppendSystemPrompt
        ? [
            `  -c "developer_instructions=$(node -e 'process.stdout.write(JSON.stringify(process.env.TB_APPEND_SYSTEM_PROMPT))')" \\`,
          ]
        : []),
      ...(options.isolateAgentConfig
        ? [
            "  --ignore-user-config \\",
            "  --ignore-rules \\",
            "  -c project_doc_max_bytes=0 \\",
          ]
        : []),
      `  "$(cat ${options.instructionPath})" \\`,
      `  2>&1 </dev/null | stdbuf -oL tee -a ${options.logPath}`,
    ].join("\n"),
  parseRun: parseCodexStream,
};

function buildOpencodeRunScript(
  binary: "opencode" | "kilo",
  options: OriRunScriptOptions
): string {
  const envPrefix = binary.toUpperCase();
  const configEnv = `${envPrefix}_CONFIG_CONTENT`;
  const configScript = [
    "const config = {",
    `  provider: { openrouter: { options: { baseURL: process.env.${GENERATION_PROXY_BASE_URL_ENV}, headers: { "X-Session-Id": "{env:ORI_OPENROUTER_SESSION_ID}" } } } },`,
    ...(options.hasAppendSystemPrompt
      ? [`  instructions: [${JSON.stringify(OPENCODE_APPEND_PROMPT_PATH)}],`]
      : []),
    ...(options.hasDisallowedTools
      ? [
          '  tools: Object.fromEntries(process.env.TB_DISALLOWED_TOOLS.split(" ").filter(Boolean).map((tool) => [tool, false])),',
        ]
      : []),
    "};",
    "process.stdout.write(JSON.stringify(config));",
  ].join(" ");
  return [
    "set -euo pipefail",
    "export HOME=/root",
    "mkdir -p /logs/agent",
    ...(options.hasSystemPrompt || options.hasAllowedTools
      ? [
          `echo "${binary} does not support systemPrompt or allowedTools" >&2`,
          "exit 2",
        ]
      : []),
    ...(options.hasAppendSystemPrompt
      ? [
          `printf '%s' "$TB_APPEND_SYSTEM_PROMPT" > ${OPENCODE_APPEND_PROMPT_PATH}`,
        ]
      : []),
    ...(options.isolateAgentConfig
      ? [
          `export ${envPrefix}_DISABLE_PROJECT_CONFIG=1 ${envPrefix}_DISABLE_CLAUDE_CODE=1 ${envPrefix}_DISABLE_EXTERNAL_SKILLS=1`,
        ]
      : []),
    ...buildGenerationProxyPrelude(options.logPath),
    `export ${configEnv}="$(node -e '${configScript}')"`,
    `ori ${binary} --model "$TB_MODEL" \\`,
    `  --reasoning-effort ${options.reasoningEffort} -- \\`,
    "  run --format json --auto \\",
    ...(options.isolateAgentConfig ? ["  --pure \\"] : []),
    `  "$(cat ${options.instructionPath})" \\`,
    `  2>&1 </dev/null | stdbuf -oL tee -a ${options.logPath}`,
  ].join("\n");
}

const OPENCODE_HARNESS: OriHarnessDef = {
  id: "opencode",
  defaultPackage: DEFAULT_OPENCODE_PACKAGE,
  binaryName: "opencode",
  remoteLogPath: "/logs/agent/opencode.txt",
  imageBuildSteps: (options) =>
    buildImageSteps({ ...options, binaryName: "opencode" }),
  buildBootstrapScript: (options) =>
    buildBootstrapScript({ ...options, binaryName: "opencode" }),
  buildRunScript: (options) => buildOpencodeRunScript("opencode", options),
  parseRun: parseOpencodeStream,
};

const KILO_HARNESS: OriHarnessDef = {
  id: "kilo",
  defaultPackage: DEFAULT_KILO_PACKAGE,
  binaryName: "kilo",
  remoteLogPath: "/logs/agent/kilo.txt",
  imageBuildSteps: (options) =>
    buildImageSteps({ ...options, binaryName: "kilo" }),
  buildBootstrapScript: (options) =>
    buildBootstrapScript({ ...options, binaryName: "kilo" }),
  buildRunScript: (options) => buildOpencodeRunScript("kilo", options),
  parseRun: parseOpencodeStream,
};

export const ORI_HARNESSES: Readonly<Record<OriAgent, OriHarnessDef>> = {
  claude: CLAUDE_HARNESS,
  pi: ORI_PI_HARNESS,
  "prime-agent": PRIME_AGENT_HARNESS,
  omp: OMP_HARNESS,
  codex: CODEX_HARNESS,
  opencode: OPENCODE_HARNESS,
  kilo: KILO_HARNESS,
};

export function getOriHarness(agent: OriAgent): OriHarnessDef {
  return ORI_HARNESSES[agent];
}

function reasoningTokensOf(usage: Record<string, unknown>): number {
  const reasoning = usage["reasoning"];
  if (typeof reasoning === "number") {
    return reasoning;
  }
  const reasoningTokens = usage["reasoningTokens"];
  return typeof reasoningTokens === "number" ? reasoningTokens : 0;
}

function parseJsonAgentStream(stdout: string): OriAgentRun {
  let inputTokens = 0;
  let outputTokens = 0;
  let cacheRead = 0;
  let cacheWrite = 0;
  let totalTokens = 0;
  let reasoningTokens = 0;
  let totalCost = 0;
  let turns = 0;
  let toolCalls = 0;
  let isError = false;
  let apiErrorStatus: string | undefined;
  let finalText: string | undefined;
  const generationIds: string[] = [];
  const assistantMessages: ModelMessage[] = [];
  const responseItems: ResponseItem[] = [];
  for (const line of stdout.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(trimmed));
    if (Either.isLeft(parsed) || !isRecord(parsed.right)) {
      continue;
    }
    const event = parsed.right;
    responseItems.push(event);
    const eventType = event["type"];
    if (eventType === "turn_end") {
      turns++;
      continue;
    }
    if (eventType === "tool_execution_end") {
      toolCalls++;
      continue;
    }
    if (eventType !== "message_end") {
      continue;
    }
    const { message } = event;
    if (!isRecord(message) || message["role"] !== "assistant") {
      continue;
    }
    const responseId = message["responseId"];
    if (
      typeof responseId === "string" &&
      responseId.length > 0 &&
      !generationIds.includes(responseId)
    ) {
      generationIds.push(responseId);
    }
    const errorMessage = message["errorMessage"];
    if (typeof errorMessage === "string" && errorMessage.length > 0) {
      isError = true;
      apiErrorStatus = errorMessage;
    } else if (
      message["stopReason"] === "error" ||
      message["stopReason"] === "aborted"
    ) {
      isError = true;
    }
    const text = textFromContent(message["content"]);
    const reasoning = reasoningFromContent(message["content"]);
    const model = optionalStringField(message, "model");
    if (text.length > 0) {
      finalText = text;
    }
    if (text.length > 0 || reasoning !== undefined) {
      assistantMessages.push(
        definedValues({
          role: MessageRole.Assistant,
          content: text,
          reasoning,
          model,
        })
      );
    }
    const { usage } = message;
    if (!isRecord(usage)) {
      continue;
    }
    const eventInputTokens =
      typeof usage["input"] === "number" ? usage["input"] : 0;
    const eventOutputTokens =
      typeof usage["output"] === "number" ? usage["output"] : 0;
    const eventCacheRead =
      typeof usage["cacheRead"] === "number" ? usage["cacheRead"] : 0;
    const eventCacheWrite =
      typeof usage["cacheWrite"] === "number" ? usage["cacheWrite"] : 0;
    const eventTotalTokens =
      typeof usage["totalTokens"] === "number" ? usage["totalTokens"] : 0;
    inputTokens += eventInputTokens;
    outputTokens += eventOutputTokens;
    cacheRead += eventCacheRead;
    cacheWrite += eventCacheWrite;
    totalTokens +=
      eventTotalTokens !== 0
        ? eventTotalTokens
        : eventInputTokens +
          eventOutputTokens +
          eventCacheRead +
          eventCacheWrite;
    reasoningTokens += reasoningTokensOf(usage);
    const { cost } = usage;
    if (isRecord(cost)) {
      totalCost += typeof cost["total"] === "number" ? cost["total"] : 0;
    }
  }
  const hasTokens = totalTokens !== 0;
  return {
    usage: hasTokens
      ? {
          inputTokens: inputTokens + cacheRead + cacheWrite,
          outputTokens,
          totalTokens,
          reasoningTokens,
          totalCost,
        }
      : undefined,
    generationIds,
    generationTimeMs: undefined,
    finalText,
    assistantMessages,
    responseItems,
    isError,
    apiErrorStatus,
    turns: turns > 0 ? turns : undefined,
    toolCalls,
  };
}

function parseJsonLines(stdout: string): Record<string, unknown>[] {
  const events: Record<string, unknown>[] = [];
  for (const line of stdout.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(trimmed));
    if (Either.isRight(parsed) && isRecord(parsed.right)) {
      events.push(parsed.right);
    }
  }
  return events;
}

const CODEX_TOOL_ITEM_TYPES = new Set([
  "command_execution",
  "file_change",
  "mcp_tool_call",
  "web_search",
]);

function parseCodexStream(stdout: string): OriAgentRun {
  let inputTokens = 0;
  let outputTokens = 0;
  let reasoningTokens = 0;
  let turns = 0;
  let toolCalls = 0;
  let isError = false;
  let apiErrorStatus: string | undefined;
  let finalText: string | undefined;
  const assistantMessages: ModelMessage[] = [];
  const responseItems: ResponseItem[] = [];
  for (const event of parseJsonLines(stdout)) {
    responseItems.push(event);
    const eventType = event["type"];
    if (eventType === "error" || eventType === "turn.failed") {
      isError = true;
      const errorRecord = isRecord(event["error"]) ? event["error"] : event;
      apiErrorStatus =
        optionalStringField(errorRecord, "message") ?? apiErrorStatus;
      continue;
    }
    if (eventType === "turn.completed") {
      turns++;
      const { usage } = event;
      if (isRecord(usage)) {
        inputTokens += optionalNumberField(usage, "input_tokens") ?? 0;
        outputTokens += optionalNumberField(usage, "output_tokens") ?? 0;
        reasoningTokens +=
          optionalNumberField(usage, "reasoning_output_tokens") ?? 0;
      }
      continue;
    }
    if (eventType !== "item.completed") {
      continue;
    }
    const { item } = event;
    if (!isRecord(item)) {
      continue;
    }
    const itemType = item["type"];
    if (typeof itemType === "string" && CODEX_TOOL_ITEM_TYPES.has(itemType)) {
      toolCalls++;
      continue;
    }
    if (itemType !== "agent_message") {
      continue;
    }
    const text = optionalStringField(item, "text");
    if (text !== undefined && text.length > 0) {
      finalText = text;
      assistantMessages.push({ role: MessageRole.Assistant, content: text });
    }
  }
  const totalTokens = inputTokens + outputTokens;
  return {
    usage:
      totalTokens !== 0
        ? {
            inputTokens,
            outputTokens,
            totalTokens,
            reasoningTokens,
            totalCost: 0,
          }
        : undefined,
    generationIds: parseProxyGenerationIds(stdout),
    generationTimeMs: undefined,
    finalText,
    assistantMessages,
    responseItems,
    isError,
    apiErrorStatus,
    turns: turns > 0 ? turns : undefined,
    toolCalls,
  };
}

function parseOpencodeStream(stdout: string): OriAgentRun {
  let inputTokens = 0;
  let outputTokens = 0;
  let cacheRead = 0;
  let cacheWrite = 0;
  let totalTokens = 0;
  let reasoningTokens = 0;
  let totalCost = 0;
  let turns = 0;
  let toolCalls = 0;
  let isError = false;
  let apiErrorStatus: string | undefined;
  let finalText: string | undefined;
  const assistantMessages: ModelMessage[] = [];
  const responseItems: ResponseItem[] = [];
  for (const event of parseJsonLines(stdout)) {
    responseItems.push(event);
    const eventType = event["type"];
    if (eventType === "error") {
      isError = true;
      const error = event["error"];
      if (isRecord(error)) {
        const data = isRecord(error["data"]) ? error["data"] : error;
        apiErrorStatus =
          optionalStringField(data, "message") ??
          optionalStringField(error, "name") ??
          apiErrorStatus;
      }
      continue;
    }
    if (eventType === "tool_use") {
      toolCalls++;
      continue;
    }
    const { part } = event;
    if (!isRecord(part)) {
      continue;
    }
    if (eventType === "text") {
      const text = optionalStringField(part, "text");
      if (text !== undefined && text.length > 0) {
        finalText = text;
        assistantMessages.push({ role: MessageRole.Assistant, content: text });
      }
      continue;
    }
    if (eventType !== "step_finish") {
      continue;
    }
    turns++;
    totalCost += optionalNumberField(part, "cost") ?? 0;
    const { tokens } = part;
    if (!isRecord(tokens)) {
      continue;
    }
    const stepInput = optionalNumberField(tokens, "input") ?? 0;
    const stepOutput = optionalNumberField(tokens, "output") ?? 0;
    const stepReasoning = optionalNumberField(tokens, "reasoning") ?? 0;
    const { cache } = tokens;
    const stepCacheRead = isRecord(cache)
      ? (optionalNumberField(cache, "read") ?? 0)
      : 0;
    const stepCacheWrite = isRecord(cache)
      ? (optionalNumberField(cache, "write") ?? 0)
      : 0;
    inputTokens += stepInput;
    outputTokens += stepOutput + stepReasoning;
    cacheRead += stepCacheRead;
    cacheWrite += stepCacheWrite;
    reasoningTokens += stepReasoning;
    totalTokens +=
      optionalNumberField(tokens, "total") ??
      stepInput + stepCacheRead + stepCacheWrite + stepOutput + stepReasoning;
  }
  return {
    usage:
      totalTokens !== 0
        ? {
            inputTokens: inputTokens + cacheRead + cacheWrite,
            outputTokens,
            totalTokens,
            reasoningTokens,
            totalCost,
          }
        : undefined,
    generationIds: parseProxyGenerationIds(stdout),
    generationTimeMs: undefined,
    finalText,
    assistantMessages,
    responseItems,
    isError,
    apiErrorStatus,
    turns: turns > 0 ? turns : undefined,
    toolCalls,
  };
}
