import { join } from "node:path";

import { gen, tryPromise } from "effect/Effect";

import type { ChatMessage, ModelUsage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import { readTerminalBenchMeta } from "./dataset";
import { buildPiModelsJson } from "./pi-custom-models";
import type { SandboxSessionFactory } from "./sandbox";
import type { PiThinkingLevel } from "./schema";
import { DEFAULT_PI_PACKAGE, DEFAULT_PI_THINKING } from "./schema";
import { ensureTasksCheckedOut } from "./tasks-source";

export interface TerminalBenchSolverOpts {
  readonly model: string;
  readonly apiKey: string;
  readonly sessionId?: string;
  readonly endpointId?: string;
  readonly thinking?: PiThinkingLevel;
  readonly piPackage?: string;
  readonly appendSystemPrompt?: string;
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
      const session = yield* sessionFactory.create({
        imageTag: meta.dockerImage,
        maxAgentTimeoutSec: meta.maxAgentTimeoutSec,
        maxTestTimeoutSec: meta.maxTestTimeoutSec,
        testDir: join(tasksDir, meta.taskId, "tests"),
        testScript: join(tasksDir, meta.taskId, "tests", "test.sh"),
        instructionPath: join(tasksDir, meta.taskId, "instruction.md"),
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
        if (piModelsJson !== undefined) {
          piEnv["TB_PI_MODELS_JSON"] = piModelsJson;
        }
        const piRun = yield* session.exec(
          [
            "bash",
            "-c",
            buildPiRunScript(
              thinking,
              opts.appendSystemPrompt !== undefined,
              piModelsJson !== undefined
            ),
          ],
          piEnv,
          meta.maxAgentTimeoutSec * 1000 + 30000
        );
        eventStream = piRun.stdout;
        agentUsage = parseUsageFromEventStream(eventStream);
        if (piRun.exitCode !== 0) {
          piExitDetail = `pi exited ${piRun.exitCode}. last output: ${eventStream.slice(-500)}`;
        }
        const testResult = yield* session.runTests();
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
            metadata: { ...state.sample.metadata, reward, testOutput },
          },
          messages,
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
            generationTimeMs: 0,
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

function buildPiRunScript(
  thinking: PiThinkingLevel,
  hasAppendSystemPrompt: boolean,
  hasPiModelsJson: boolean
): string {
  return [
    "set -euo pipefail",
    ...(hasPiModelsJson
      ? [
          "mkdir -p ~/.pi/agent",
          "printf '%s' \"$TB_PI_MODELS_JSON\" > ~/.pi/agent/models.json",
        ]
      : []),
    "pi --print --mode json --no-session \\",
    '  --provider "$TB_PROVIDER" --model "$TB_MODEL" \\',
    `  --thinking ${thinking} \\`,
    ...(hasAppendSystemPrompt
      ? ['  --append-system-prompt "$TB_APPEND_SYSTEM_PROMPT" \\']
      : []),
    '  "$(cat /instruction.md)" \\',
    `  2>&1 </dev/null | grep -v '"type":"message_update"' | stdbuf -oL tee ${REMOTE_AGENT_LOG}`,
  ].join("\n");
}

function parseUsageFromEventStream(
  eventStream: string
): ModelUsage | undefined {
  let inputTokens = 0;
  let outputTokens = 0;
  let cacheRead = 0;
  let cacheWrite = 0;
  let totalCost = 0;
  for (const line of eventStream.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "") {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(trimmed));
    if (
      Either.isLeft(parsed) ||
      !isRecord(parsed.right) ||
      parsed.right["type"] !== "message_end"
    ) {
      continue;
    }
    const { message } = parsed.right;
    if (!isRecord(message) || message["role"] !== "assistant") {
      continue;
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
    const { cost } = usage;
    if (isRecord(cost)) {
      totalCost += typeof cost["total"] === "number" ? cost["total"] : 0;
    }
  }
  if (
    inputTokens === 0 &&
    outputTokens === 0 &&
    cacheRead === 0 &&
    cacheWrite === 0
  ) {
    return undefined;
  }
  return {
    inputTokens: inputTokens + cacheRead + cacheWrite,
    outputTokens,
    totalTokens: inputTokens + outputTokens + cacheRead + cacheWrite,
    reasoningTokens: 0,
    totalCost,
  };
}
