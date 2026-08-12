import { forEach, gen, tryPromise } from "effect/Effect";

import type { ChatMessage, ModelUsage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import { recordGenerationId } from "../../runtime/generation-ids";
import type { SandboxSessionFactory } from "../harbor/sandbox";
import { readTerminalBenchMeta } from "./dataset";
import type { OriHarnessDef } from "./ori-harness";
import type { ClaudeEffortLevel } from "./schema";
import { DEFAULT_CLAUDE_EFFORT, DEFAULT_ORI_INSTALL_URL } from "./schema";
import {
  createTerminalBenchSession,
  runTerminalBenchVerifier,
} from "./session";
import { ensureTasksCheckedOut } from "./tasks-source";

const AGENT_TIMEOUT_MARGIN_MS = 30000;

const EXIT_DETAIL_TAIL_CHARS = 500;

const ZERO_USAGE: ModelUsage = {
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  reasoningTokens: 0,
  totalCost: 0,
};

export interface OriSolverOpts {
  readonly model: string;
  readonly apiKey: string;
  readonly sessionId?: string;
  readonly endpointId?: string;
  readonly agentPackage?: string;
  readonly oriInstallUrl?: string;
  readonly appendSystemPrompt?: string;
  readonly systemPrompt?: string;
  readonly effort?: ClaudeEffortLevel;
  readonly allowedTools?: readonly string[];
  readonly disallowedTools?: readonly string[];
  readonly isolateAgentConfig?: boolean;
}

export function oriSolver(
  sessionFactory: SandboxSessionFactory,
  opts: OriSolverOpts,
  harness: OriHarnessDef
): SolverService {
  const agentPackage = opts.agentPackage ?? harness.defaultPackage;
  const oriInstallUrl = opts.oriInstallUrl ?? DEFAULT_ORI_INSTALL_URL;
  const effort = opts.effort ?? DEFAULT_CLAUDE_EFFORT;
  const allowedTools = opts.allowedTools ?? [];
  const disallowedTools = opts.disallowedTools ?? [];
  const hasAllowedTools = allowedTools.length > 0;
  const hasDisallowedTools = disallowedTools.length > 0;
  return (state) =>
    gen(function* () {
      const meta = readTerminalBenchMeta(state.sample.metadata);
      if (meta === undefined) {
        return yield* new SolverError({
          message: `terminal-bench ori solver received a sample without terminal-bench metadata (id=${state.sample.id})`,
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
        imageBuildSteps: harness.imageBuildSteps(agentPackage, oriInstallUrl),
      });
      try {
        const agentEnv: Record<string, string> = {
          OPENROUTER_API_KEY: opts.apiKey,
          TB_MODEL: opts.model,
        };
        if (opts.endpointId !== undefined) {
          agentEnv["OPENROUTER_ENDPOINT_ID"] = opts.endpointId;
        }
        if (opts.sessionId !== undefined) {
          agentEnv["OPENROUTER_SESSION_ID"] = opts.sessionId;
        }
        if (opts.appendSystemPrompt !== undefined) {
          agentEnv["TB_APPEND_SYSTEM_PROMPT"] = opts.appendSystemPrompt;
        }
        if (opts.systemPrompt !== undefined) {
          agentEnv["TB_SYSTEM_PROMPT"] = opts.systemPrompt;
        }
        if (hasAllowedTools) {
          agentEnv["TB_ALLOWED_TOOLS"] = allowedTools.join(" ");
        }
        if (hasDisallowedTools) {
          agentEnv["TB_DISALLOWED_TOOLS"] = disallowedTools.join(" ");
        }
        const agentRun = yield* session.exec(
          [
            "bash",
            "-c",
            harness.buildRunScript({
              effort,
              hasSystemPrompt: opts.systemPrompt !== undefined,
              hasAppendSystemPrompt: opts.appendSystemPrompt !== undefined,
              hasAllowedTools,
              hasDisallowedTools,
              isolateAgentConfig: opts.isolateAgentConfig === true,
            }),
          ],
          agentEnv,
          meta.maxAgentTimeoutSec * 1000 + AGENT_TIMEOUT_MARGIN_MS
        );
        const eventStream = agentRun.stdout;
        const parsed = harness.parseRun(eventStream);
        yield* forEach(parsed.generationIds, (id) => recordGenerationId(id), {
          discard: true,
        });
        const failureDetail = buildFailureDetail({
          agentId: harness.id,
          exitCode: agentRun.exitCode,
          isError: parsed.isError,
          apiErrorStatus: parsed.apiErrorStatus,
          eventStream,
        });
        const testResult = yield* runTerminalBenchVerifier(session, meta);
        const { reward } = testResult;
        const testOutput = failureDetail
          ? `${failureDetail}\n\n${testResult.output}`
          : testResult.output;
        const completion = parsed.finalText ?? eventStream;
        const messages: ChatMessage[] = [
          { role: MessageRole.User, content: state.sample.input },
          ...parsed.assistantMessages,
        ];
        return {
          sample: {
            ...state.sample,
            metadata: {
              ...state.sample.metadata,
              reward,
              testOutput,
              agent: harness.id,
              agentExitCode: agentRun.exitCode,
              agentIsError: parsed.isError,
              generationIds: parsed.generationIds,
              ...(parsed.turns !== undefined && { agentTurns: parsed.turns }),
              agentToolCalls: parsed.toolCalls,
            },
          },
          messages,
          responseItems: parsed.responseItems,
          output: {
            completion,
            message: { role: MessageRole.Assistant, content: completion },
            usage: parsed.usage ?? ZERO_USAGE,
            generationTimeMs: parsed.generationTimeMs ?? 0,
          },
          completed: true,
        };
      } finally {
        yield* session.destroy();
      }
    });
}

function buildFailureDetail(input: {
  agentId: string;
  exitCode: number;
  isError: boolean;
  apiErrorStatus: string | undefined;
  eventStream: string;
}): string {
  const { agentId, exitCode, isError, apiErrorStatus, eventStream } = input;
  if (exitCode === 0 && !isError && apiErrorStatus === undefined) {
    return "";
  }
  const reasons: string[] = [];
  if (exitCode !== 0) {
    reasons.push(`exited ${exitCode}`);
  }
  if (isError) {
    reasons.push("reported is_error=true");
  }
  if (apiErrorStatus !== undefined) {
    reasons.push(`api_error_status=${apiErrorStatus}`);
  }
  return `${agentId} ${reasons.join(", ")}. last output: ${eventStream.slice(-EXIT_DETAIL_TAIL_CHARS)}`;
}
