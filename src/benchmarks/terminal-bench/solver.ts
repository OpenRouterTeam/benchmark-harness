import { currentTimeMillis } from "effect/Clock";
import { forEach, gen, tryPromise } from "effect/Effect";

import type { ChatMessage, ModelUsage, ResponseItem } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import { recordGenerationId } from "../../runtime/generation-ids";
import type { SandboxSessionFactory } from "../harbor/sandbox";
import { readTerminalBenchMeta } from "./dataset";
import { buildPiModelsJson } from "./pi-custom-models";
import type { PiThinkingLevel } from "./schema";
import { DEFAULT_PI_PACKAGE, DEFAULT_PI_THINKING } from "./schema";
import {
  createTerminalBenchSession,
  runTerminalBenchVerifier,
} from "./session";
import { ensureTasksCheckedOut } from "./tasks-source";

export interface TerminalBenchSolverOpts {
  readonly model: string;
  readonly apiKey: string;
  readonly sessionId?: string;
  readonly endpointId?: string;
  readonly thinking?: PiThinkingLevel;
  readonly piPackage?: string;
  readonly appendSystemPrompt?: string;
  readonly systemPrompt?: string;
  readonly allowedTools?: readonly string[];
  readonly disallowedTools?: readonly string[];
  readonly isolateAgentConfig?: boolean;
}

const NODE_VERSION = "22" as const;

const NVM_INSTALL_URL =
  "https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.2/install.sh" as const;

const REMOTE_AGENT_LOG = "/logs/agent/pi.txt" as const;

export function piSolver(
  sessionFactory: SandboxSessionFactory,
  opts: TerminalBenchSolverOpts
): SolverService {
  const thinking = opts.thinking ?? DEFAULT_PI_THINKING;
  const piPackage = opts.piPackage ?? DEFAULT_PI_PACKAGE;
  const [provider, modelId] = parseModel(opts.model);
  const piModelsJson = buildPiModelsJson(provider, modelId, opts.sessionId);
  const allowedTools = opts.allowedTools ?? [];
  const disallowedTools = opts.disallowedTools ?? [];
  const hasAllowedTools = allowedTools.length > 0;
  const hasDisallowedTools = disallowedTools.length > 0;
  return (state) =>
    gen(function* () {
      const meta = readTerminalBenchMeta(state.sample.metadata);
      if (meta === undefined) {
        return yield* new SolverError({
          message: `terminal-bench solver received a sample without terminal-bench metadata (id=${state.sample.id})`,
        });
      }
      const tasksDir = yield* tryPromise({
        try: () => ensureTasksCheckedOut(),
        catch: (e: unknown) =>
          new SolverError({
            message: `Failed to check out terminal-bench tasks: ${String(e)}`,
          }),
      });
      const session = yield* createTerminalBenchSession({
        sessionFactory,
        meta,
        tasksDir,
        imageBuildSteps: buildPiImageSteps(piPackage),
      });
      let reward = 0;
      let testOutput = "";
      let agentUsage: ModelUsage | undefined;
      let eventStream = "";
      let piExitDetail = "";
      try {
        const piEnv: Record<string, string> = {
          OPENROUTER_API_KEY: opts.apiKey,
          TB_PROVIDER: provider,
          TB_MODEL: modelId,
        };
        if (opts.endpointId !== undefined) {
          piEnv["OPENROUTER_ENDPOINT_ID"] = opts.endpointId;
        }
        if (opts.appendSystemPrompt !== undefined) {
          piEnv["TB_APPEND_SYSTEM_PROMPT"] = opts.appendSystemPrompt;
        }
        if (opts.systemPrompt !== undefined) {
          piEnv["TB_SYSTEM_PROMPT"] = opts.systemPrompt;
        }
        if (hasAllowedTools) {
          piEnv["TB_ALLOWED_TOOLS"] = allowedTools.join(",");
        }
        if (hasDisallowedTools) {
          piEnv["TB_DISALLOWED_TOOLS"] = disallowedTools.join(",");
        }
        if (piModelsJson !== undefined) {
          piEnv["TB_PI_MODELS_JSON"] = piModelsJson;
        }
        const startedAt = yield* currentTimeMillis;
        const piRun = yield* session.exec(
          [
            "bash",
            "-c",
            buildPiRunScript({
              thinking,
              hasSystemPrompt: opts.systemPrompt !== undefined,
              hasAppendSystemPrompt: opts.appendSystemPrompt !== undefined,
              hasPiModelsJson: piModelsJson !== undefined,
              hasAllowedTools,
              hasDisallowedTools,
              isolateAgentConfig: opts.isolateAgentConfig === true,
            }),
          ],
          piEnv,
          meta.maxAgentTimeoutSec * 1000 + 30000
        );
        const finishedAt = yield* currentTimeMillis;
        eventStream = piRun.stdout;
        const piParse = parsePiEventStream(eventStream);
        agentUsage = piParse.usage;
        yield* forEach(piParse.generationIds, (id) => recordGenerationId(id), {
          discard: true,
        });
        const failureReasons: string[] = [];
        if (piRun.exitCode !== 0) {
          failureReasons.push(`exited ${piRun.exitCode}`);
        }
        if (piParse.apiErrors.length > 0) {
          failureReasons.push(`api errors: ${piParse.apiErrors.join("; ")}`);
        }
        if (failureReasons.length > 0) {
          piExitDetail = `pi ${failureReasons.join(", ")}. last output: ${eventStream.slice(-500)}`;
        }
        const testResult = yield* runTerminalBenchVerifier(session, meta);
        ({ reward } = testResult);
        testOutput = piExitDetail
          ? `${piExitDetail}\n\n${testResult.output}`
          : testResult.output;
        const messages: ChatMessage[] = [
          { role: MessageRole.User, content: state.sample.input },
          { role: MessageRole.Assistant, content: eventStream },
        ];
        return {
          sample: {
            ...state.sample,
            metadata: {
              ...state.sample.metadata,
              reward,
              testOutput,
              agent: "pi",
              agentExitCode: piRun.exitCode,
              agentIsError: piParse.apiErrors.length > 0,
              generationIds: piParse.generationIds,
              agentTurns: piParse.turns,
              agentToolCalls: piParse.toolCalls,
            },
          },
          messages,
          responseItems: piParse.responseItems,
          output: {
            completion: eventStream,
            message: { role: MessageRole.Assistant, content: eventStream },
            usage: agentUsage ?? {
              inputTokens: 0,
              outputTokens: 0,
              totalTokens: 0,
              reasoningTokens: 0,
              totalCost: 0,
            },
            generationTimeMs: finishedAt - startedAt,
          },
          completed: true,
        };
      } finally {
        yield* session.destroy();
      }
    });
}

function buildPiImageSteps(piPackage: string): string[] {
  return [
    "RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates",
    "ENV NVM_DIR=/root/.nvm",
    `RUN curl -o- ${NVM_INSTALL_URL} | bash`,
    `RUN . /root/.nvm/nvm.sh && nvm install ${NODE_VERSION} && npm install -g ${piPackage} && ln -sf $(which pi) /usr/local/bin/pi && ln -sf $(which node) /usr/local/bin/node && ln -sf $(which npm) /usr/local/bin/npm`,
    "RUN pi --version",
  ];
}

export function parseModel(model: string): readonly [string, string] {
  const idx = model.indexOf("/");
  if (idx <= 0 || idx === model.length - 1) {
    throw new Error(
      `terminal-bench pi solver requires a model in "provider/model" form (got "${model}")`
    );
  }
  const provider = model.slice(0, idx);
  const modelId = model.slice(idx + 1);
  if (provider === "openrouter" && !modelId.includes("/")) {
    return [provider, `openrouter/${modelId}`] as const;
  }
  return [provider, modelId] as const;
}

export interface PiRunScriptOptions {
  readonly thinking: PiThinkingLevel;
  readonly hasSystemPrompt: boolean;
  readonly hasAppendSystemPrompt: boolean;
  readonly hasPiModelsJson: boolean;
  readonly hasAllowedTools: boolean;
  readonly hasDisallowedTools: boolean;
  readonly isolateAgentConfig: boolean;
}

function buildPiRunScript(options: PiRunScriptOptions): string {
  return [
    "set -euo pipefail",
    ...(options.hasPiModelsJson
      ? [
          "mkdir -p ~/.pi/agent",
          "printf '%s' \"$TB_PI_MODELS_JSON\" > ~/.pi/agent/models.json",
        ]
      : []),
    "pi --print --mode json --no-session \\",
    '  --provider "$TB_PROVIDER" --model "$TB_MODEL" \\',
    `  --thinking ${options.thinking} \\`,
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
    '  "$(cat /instruction.md)" \\',
    `  2>&1 </dev/null | grep -v '"type":"message_update"' | stdbuf -oL tee ${REMOTE_AGENT_LOG}`,
  ].join("\n");
}

export interface PiEventStreamParse {
  readonly usage: ModelUsage | undefined;
  readonly generationIds: readonly string[];
  readonly apiErrors: readonly string[];
  readonly turns: number;
  readonly toolCalls: number;
  readonly responseItems: readonly ResponseItem[];
}

export function parsePiEventStream(eventStream: string): PiEventStreamParse {
  let inputTokens = 0;
  let outputTokens = 0;
  let cacheRead = 0;
  let cacheWrite = 0;
  let reasoningTokens = 0;
  let totalCost = 0;
  let turns = 0;
  let toolCalls = 0;
  const generationIds: string[] = [];
  const apiErrors: string[] = [];
  const responseItems: ResponseItem[] = [];
  for (const line of eventStream.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "") {
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
      apiErrors.push(errorMessage);
    } else if (message["stopReason"] === "error") {
      apiErrors.push("pi reported stopReason=error without an errorMessage");
    }
    const { usage } = message;
    if (!isRecord(usage)) {
      continue;
    }
    inputTokens += typeof usage["input"] === "number" ? usage["input"] : 0;
    outputTokens += typeof usage["output"] === "number" ? usage["output"] : 0;
    cacheRead +=
      typeof usage["cacheRead"] === "number" ? usage["cacheRead"] : 0;
    cacheWrite +=
      typeof usage["cacheWrite"] === "number" ? usage["cacheWrite"] : 0;
    reasoningTokens +=
      typeof usage["reasoning"] === "number" ? usage["reasoning"] : 0;
    const { cost } = usage;
    if (isRecord(cost)) {
      totalCost += typeof cost["total"] === "number" ? cost["total"] : 0;
    }
  }
  const hasTokens =
    inputTokens !== 0 ||
    outputTokens !== 0 ||
    cacheRead !== 0 ||
    cacheWrite !== 0;
  return {
    usage: hasTokens
      ? {
          inputTokens: inputTokens + cacheRead + cacheWrite,
          outputTokens,
          totalTokens: inputTokens + outputTokens + cacheRead + cacheWrite,
          reasoningTokens,
          totalCost,
        }
      : undefined,
    generationIds,
    apiErrors,
    turns,
    toolCalls,
    responseItems,
  };
}
