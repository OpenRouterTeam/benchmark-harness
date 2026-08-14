import { currentTimeMillis } from "effect/Clock";
import type { Effect } from "effect/Effect";
import { forEach, gen } from "effect/Effect";

import type { SolverError } from "../../harness/core";
import { recordGenerationId } from "../../runtime/generation-ids";
import type { SandboxSessionInstance } from "../harbor/sandbox";
import type { OriAgentRun, OriHarnessDef } from "./harness";
import type { OriChannel, OriReasoningEffort } from "./schema";
import {
  DEFAULT_ORI_CHANNEL,
  DEFAULT_ORI_INSTALL_URL,
  DEFAULT_ORI_REASONING_EFFORT,
} from "./schema";

const EXIT_DETAIL_TAIL_CHARS = 500;

export interface AgentCliOpts {
  readonly model: string;
  readonly apiKey: string;
  readonly sessionId?: string;
  readonly endpointId?: string;
  readonly agentPackage?: string;
  readonly oriInstallUrl?: string;
  readonly systemPrompt?: string;
  readonly appendSystemPrompt?: string;
  readonly agentReasoningEffort?: OriReasoningEffort;
  readonly oriChannel?: OriChannel;
  readonly allowedTools?: readonly string[];
  readonly disallowedTools?: readonly string[];
  readonly isolateAgentConfig?: boolean;
}

export interface AgentCliRunResult extends OriAgentRun {
  readonly exitCode: number;
  readonly rawStream: string;
  readonly failureDetail: string;
}

export function buildAgentCliEnv(opts: AgentCliOpts): Record<string, string> {
  const env: Record<string, string> = {
    OPENROUTER_API_KEY: opts.apiKey,
    TB_MODEL: opts.model,
  };
  if (opts.endpointId !== undefined) {
    env["OPENROUTER_ENDPOINT_ID"] = opts.endpointId;
  }
  if (opts.sessionId !== undefined) {
    env["OPENROUTER_SESSION_ID"] = opts.sessionId;
  }
  if (opts.systemPrompt !== undefined) {
    env["TB_SYSTEM_PROMPT"] = opts.systemPrompt;
  }
  if (opts.appendSystemPrompt !== undefined) {
    env["TB_APPEND_SYSTEM_PROMPT"] = opts.appendSystemPrompt;
  }
  const allowedTools = opts.allowedTools ?? [];
  if (allowedTools.length > 0) {
    env["TB_ALLOWED_TOOLS"] = allowedTools.join(" ");
  }
  const disallowedTools = opts.disallowedTools ?? [];
  if (disallowedTools.length > 0) {
    env["TB_DISALLOWED_TOOLS"] = disallowedTools.join(" ");
  }
  return env;
}

export function agentImageBuildSteps(
  harness: OriHarnessDef,
  opts: AgentCliOpts
): string[] {
  return harness.imageBuildSteps({
    agentPackage: opts.agentPackage ?? harness.defaultPackage,
    oriInstallUrl: opts.oriInstallUrl ?? DEFAULT_ORI_INSTALL_URL,
    oriChannel: opts.oriChannel ?? DEFAULT_ORI_CHANNEL,
  });
}

export function runAgentCli(input: {
  readonly session: SandboxSessionInstance;
  readonly harness: OriHarnessDef;
  readonly opts: AgentCliOpts;
  readonly instructionPath: string;
  readonly timeoutMs: number;
}): Effect<AgentCliRunResult, SolverError> {
  const { session, harness, opts, instructionPath, timeoutMs } = input;
  const script = harness.buildRunScript({
    instructionPath,
    logPath: harness.remoteLogPath,
    reasoningEffort: opts.agentReasoningEffort ?? DEFAULT_ORI_REASONING_EFFORT,
    hasSystemPrompt: opts.systemPrompt !== undefined,
    hasAppendSystemPrompt: opts.appendSystemPrompt !== undefined,
    hasAllowedTools: (opts.allowedTools ?? []).length > 0,
    hasDisallowedTools: (opts.disallowedTools ?? []).length > 0,
    isolateAgentConfig: opts.isolateAgentConfig === true,
  });
  return gen(function* () {
    const startedAt = yield* currentTimeMillis;
    const run = yield* session.exec(
      ["bash", "-c", script],
      buildAgentCliEnv(opts),
      timeoutMs
    );
    const elapsedMs = (yield* currentTimeMillis) - startedAt;
    const parsed = harness.parseRun(run.stdout);
    yield* forEach(parsed.generationIds, (id) => recordGenerationId(id), {
      discard: true,
    });
    return {
      ...parsed,
      generationTimeMs: parsed.generationTimeMs ?? elapsedMs,
      exitCode: run.exitCode,
      rawStream: run.stdout,
      failureDetail: buildFailureDetail({
        agentId: harness.id,
        exitCode: run.exitCode,
        isError: parsed.isError,
        apiErrorStatus: parsed.apiErrorStatus,
        eventStream: run.stdout,
      }),
    };
  });
}

export function buildFailureDetail(input: {
  readonly agentId: string;
  readonly exitCode: number;
  readonly isError: boolean;
  readonly apiErrorStatus: string | undefined;
  readonly eventStream: string;
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

export function agentCliMetadata(
  harnessId: string,
  run: AgentCliRunResult
): Readonly<Record<string, unknown>> {
  return {
    agent: harnessId,
    agentExitCode: run.exitCode,
    agentIsError: run.isError,
    generationIds: run.generationIds,
    ...(run.turns !== undefined && { agentTurns: run.turns }),
    agentToolCalls: run.toolCalls,
  };
}
