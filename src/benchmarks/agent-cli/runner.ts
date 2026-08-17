import { currentTimeMillis } from "effect/Clock";
import type { Effect } from "effect/Effect";
import {
  catchAll,
  catchAllDefect,
  forEach,
  gen,
  map,
  succeed,
} from "effect/Effect";

import { SolverError } from "../../harness/core";
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

const ORI_SESSION_ID_ENV = "ORI_OPENROUTER_SESSION_ID" as const;

const AGENT_EXEC_UNAVAILABLE_EXIT = -1;

const LOG_RECOVERY_TIMEOUT_MS = 60000;

function recoverAfterExecFailure(
  session: SandboxSessionInstance,
  harness: OriHarnessDef,
  execError: string
): Effect<
  {
    readonly stdout: string;
    readonly exitCode: number;
    readonly execError: string;
  },
  never
> {
  return recoverAgentLog(session, harness.remoteLogPath).pipe(
    map((stdout) => ({
      stdout,
      exitCode: AGENT_EXEC_UNAVAILABLE_EXIT,
      execError,
    }))
  );
}

function unknownToMessage(value: unknown): string {
  return value instanceof Error ? value.message : String(value);
}

function recoverAgentLog(
  session: SandboxSessionInstance,
  logPath: string
): Effect<string, never> {
  return session.exec(["cat", logPath], {}, LOG_RECOVERY_TIMEOUT_MS).pipe(
    map((read) => read.stdout),
    catchAll(() => succeed(""))
  );
}

const CONTROL_CHAR_MAX = 0x1f;

const DELETE_CHAR = 0x7f;

export function isSafeOriSessionId(sessionId: string): boolean {
  if (sessionId.length === 0) {
    return false;
  }
  for (const char of sessionId) {
    const code = char.codePointAt(0) ?? 0;
    if (code <= CONTROL_CHAR_MAX || code === DELETE_CHAR) {
      return false;
    }
  }
  return true;
}

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

function normalizeAgentModel(model: string): string {
  const routingPrefix = "openrouter/";
  if (!model.startsWith(routingPrefix)) {
    return model;
  }
  const rest = model.slice(routingPrefix.length);
  return rest.includes("/") ? rest : model;
}

function buildAgentCliEnv(opts: AgentCliOpts): Record<string, string> {
  const sessionId =
    opts.sessionId !== undefined && isSafeOriSessionId(opts.sessionId)
      ? opts.sessionId
      : undefined;
  const joined = (values: readonly string[] | undefined): string | undefined =>
    values !== undefined && values.length > 0 ? values.join(" ") : undefined;
  const entries: readonly (readonly [string, string | undefined])[] = [
    ["OPENROUTER_API_KEY", opts.apiKey],
    ["TB_MODEL", normalizeAgentModel(opts.model)],
    ["OPENROUTER_ENDPOINT_ID", opts.endpointId],
    [ORI_SESSION_ID_ENV, sessionId],
    ["TB_SYSTEM_PROMPT", opts.systemPrompt],
    ["TB_APPEND_SYSTEM_PROMPT", opts.appendSystemPrompt],
    ["TB_ALLOWED_TOOLS", joined(opts.allowedTools)],
    ["TB_DISALLOWED_TOOLS", joined(opts.disallowedTools)],
  ];
  const env: Record<string, string> = {};
  for (const [key, value] of entries) {
    if (value !== undefined) {
      env[key] = value;
    }
  }
  return env;
}

export function buildAgentCliOpts(input: {
  readonly model: string;
  readonly apiKey: string;
  readonly endpointId?: string;
  readonly sessionId?: string;
  readonly agentCli?: AgentCliOpts;
  readonly submissionProtocol: string;
}): AgentCliOpts {
  const base: AgentCliOpts = input.agentCli ?? {
    model: input.model,
    apiKey: input.apiKey,
    ...(input.endpointId !== undefined && { endpointId: input.endpointId }),
    ...(input.sessionId !== undefined && { sessionId: input.sessionId }),
  };
  const caller = base.appendSystemPrompt;
  return {
    ...base,
    appendSystemPrompt:
      caller === undefined || caller.length === 0
        ? input.submissionProtocol
        : `${input.submissionProtocol}\n\n${caller}`,
  };
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
    if (
      opts.sessionId !== undefined &&
      opts.sessionId.length > 0 &&
      !isSafeOriSessionId(opts.sessionId)
    ) {
      return yield* new SolverError({
        message: `sessionId contains a control character, which ori replaces with a fresh UUID and silently detaches the run from its generations (id=${JSON.stringify(opts.sessionId)})`,
      });
    }
    const startedAt = yield* currentTimeMillis;
    const outcome = yield* session
      .exec(["bash", "-c", script], buildAgentCliEnv(opts), timeoutMs)
      .pipe(
        map((run) => ({ stdout: run.stdout, exitCode: run.exitCode })),
        catchAll((cause) =>
          recoverAfterExecFailure(session, harness, cause.message)
        ),
        catchAllDefect((defect) =>
          recoverAfterExecFailure(session, harness, unknownToMessage(defect))
        )
      );
    const elapsedMs = (yield* currentTimeMillis) - startedAt;
    const run = outcome;
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
        ...("execError" in run && run.execError !== undefined
          ? { execError: run.execError }
          : {}),
      }),
    };
  });
}

function buildFailureDetail(input: {
  readonly agentId: string;
  readonly exitCode: number;
  readonly isError: boolean;
  readonly apiErrorStatus: string | undefined;
  readonly eventStream: string;
  readonly execError?: string;
}): string {
  const { agentId, exitCode, isError, apiErrorStatus, eventStream, execError } =
    input;
  if (
    exitCode === 0 &&
    !isError &&
    apiErrorStatus === undefined &&
    execError === undefined
  ) {
    return "";
  }
  const reasons: string[] = [];
  if (execError !== undefined) {
    reasons.push(`exec did not complete: ${execError}`);
  }
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
