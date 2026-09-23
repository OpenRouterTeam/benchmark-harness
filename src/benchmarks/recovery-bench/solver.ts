import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { Effect } from "effect/Effect";
import { either, gen, sync, tryPromise } from "effect/Effect";

import type { ModelMessage, ModelUsage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import type {
  SandboxSessionFactory,
  SandboxSessionInstance,
} from "../../sandbox/session";
import type { OriHarnessDef } from "../agent-cli/harness";
import type { AgentCliOpts } from "../agent-cli/runner";
import {
  agentCliMetadata,
  agentImageBuildSteps,
  runAgentCli,
} from "../agent-cli/runner";
import {
  agentNetworkDeviation,
  createTerminalBenchSession,
  REMOTE_INSTRUCTION,
  runTerminalBenchVerifier,
} from "../terminal-bench/session";
import type { RecoveryBenchSampleMeta } from "./dataset";
import { readRecoveryBenchMeta } from "./dataset";
import {
  buildRecoveryInstruction,
  formatMessagesAsText,
  SUMMARY_FALLBACK,
} from "./prompts";
import type { RecoveryBenchMessageMode } from "./schema";
import { DEFAULT_REPLAY_COMMAND_TIMEOUT_SEC } from "./schema";
import { ensureTasksCheckedOut } from "./tasks-source";
import type { ParsedTrajectory } from "./trajectory";
import { parseTrajectory, planReplay } from "./trajectory";

const AGENT_TIMEOUT_MARGIN_MS = 30000;

const ZERO_USAGE: ModelUsage = {
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  reasoningTokens: 0,
  totalCost: 0,
};

export interface SummaryResult {
  readonly text: string;
  readonly usage?: ModelUsage;
  readonly generationTimeMs?: number;
}

export type Summarizer = (
  messages: readonly ModelMessage[]
) => Effect<SummaryResult, SolverError>;

export interface ResolvedMessageContext {
  readonly context: string | undefined;
  readonly summaryUsed: boolean;
  readonly summaryFellBack: boolean;
  readonly summary?: SummaryResult;
}

export type TrajectoryFetcher = (
  meta: RecoveryBenchSampleMeta
) => Effect<string, SolverError>;

export interface RecoveryBenchSolverOpts {
  readonly agentCli: AgentCliOpts;
  readonly messageMode: RecoveryBenchMessageMode;
  readonly replayCommandTimeoutSec?: number;
  readonly maxInstructionBytes: number;
  readonly fetchTrajectory: TrajectoryFetcher;
  readonly summarize?: Summarizer;
}

export interface ReplayResult {
  readonly attempted: number;
  readonly skippedInterrupted: number;
  readonly failed: number;
  readonly totalExecutable: number;
}

export function replayFailedTrajectory(
  session: SandboxSessionInstance,
  trajectory: ParsedTrajectory,
  commandTimeoutSec: number
): Effect<ReplayResult, never> {
  const plan = planReplay(trajectory.commands);
  return gen(function* () {
    let failed = 0;
    for (const command of plan.commands) {
      const result = yield* either(
        session.exec(["bash", "-lc", command], {}, commandTimeoutSec * 1000)
      );
      if (Either.isLeft(result) || result.right.exitCode !== 0) {
        failed += 1;
      }
    }
    return {
      attempted: plan.commands.length,
      skippedInterrupted: plan.skippedInterrupted,
      failed,
      totalExecutable: plan.totalExecutable,
    };
  });
}

export function resolveMessageContext(
  mode: RecoveryBenchMessageMode,
  messages: readonly ModelMessage[],
  summarize: Summarizer | undefined
): Effect<ResolvedMessageContext, SolverError> {
  return gen(function* () {
    if (mode === "none" || messages.length === 0) {
      return { context: undefined, summaryUsed: false, summaryFellBack: false };
    }
    if (mode === "full") {
      return {
        context: formatMessagesAsText(messages),
        summaryUsed: false,
        summaryFellBack: false,
      };
    }
    if (summarize === undefined) {
      return {
        context: SUMMARY_FALLBACK,
        summaryUsed: true,
        summaryFellBack: true,
      };
    }
    const summary = yield* either(summarize(messages));
    if (Either.isLeft(summary) || summary.right.text.trim().length === 0) {
      return {
        context: SUMMARY_FALLBACK,
        summaryUsed: true,
        summaryFellBack: true,
      };
    }
    return {
      context: summary.right.text,
      summaryUsed: true,
      summaryFellBack: false,
      summary: summary.right,
    };
  });
}

function addUsage(
  a: ModelUsage | undefined,
  b: ModelUsage | undefined
): ModelUsage {
  const x = a ?? ZERO_USAGE;
  const y = b ?? ZERO_USAGE;
  return {
    inputTokens: (x.inputTokens ?? 0) + (y.inputTokens ?? 0),
    outputTokens: (x.outputTokens ?? 0) + (y.outputTokens ?? 0),
    totalTokens: (x.totalTokens ?? 0) + (y.totalTokens ?? 0),
    reasoningTokens: (x.reasoningTokens ?? 0) + (y.reasoningTokens ?? 0),
    totalCost: (x.totalCost ?? 0) + (y.totalCost ?? 0),
  };
}

function uploadInstruction(
  session: SandboxSessionInstance,
  instruction: string
): Effect<void, SolverError> {
  return gen(function* () {
    const dir = yield* sync(() =>
      mkdtempSync(join(tmpdir(), "recovery-bench-instruction-"))
    );
    try {
      const localPath = join(dir, "instruction.md");
      yield* sync(() => writeFileSync(localPath, instruction));
      yield* session.uploadFile(localPath, REMOTE_INSTRUCTION);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
}

export function recoveryBenchSolver(
  sessionFactory: SandboxSessionFactory,
  opts: RecoveryBenchSolverOpts,
  harness: OriHarnessDef
): SolverService {
  const replayTimeoutSec =
    opts.replayCommandTimeoutSec ?? DEFAULT_REPLAY_COMMAND_TIMEOUT_SEC;
  return (state) =>
    gen(function* () {
      const meta = readRecoveryBenchMeta(state.sample.metadata);
      if (meta === undefined) {
        return yield* new SolverError({
          message: `recovery-bench solver received a sample without recovery-bench metadata (id=${state.sample.id})`,
        });
      }

      const trajectoryText = yield* opts.fetchTrajectory(meta);
      const trajectory = parseTrajectory(trajectoryText);
      if (Either.isLeft(trajectory)) {
        return yield* new SolverError({
          message: `recovery-bench trial ${meta.trialDir}: ${trajectory.left.message}`,
        });
      }

      const resolved = yield* resolveMessageContext(
        opts.messageMode,
        trajectory.right.messages,
        opts.summarize
      );
      const instruction = buildRecoveryInstruction(
        state.sample.input,
        resolved.context
      );
      const instructionBytes = Buffer.byteLength(instruction, "utf8");
      if (instructionBytes > opts.maxInstructionBytes) {
        return yield* new SolverError({
          message: `recovery-bench trial ${meta.trialDir}: recovery instruction is ${instructionBytes} bytes, limit is ${opts.maxInstructionBytes}`,
        });
      }

      const replayBudgetSec =
        planReplay(trajectory.right.commands).commands.length *
        replayTimeoutSec;

      const tasksDir = yield* tryPromise({
        try: () => ensureTasksCheckedOut(),
        catch: (e: unknown) =>
          new SolverError({
            message: `Failed to check out Terminal-Bench 2.0 tasks: ${String(e)}`,
          }),
      });

      const session = yield* createTerminalBenchSession({
        sessionFactory,
        meta,
        tasksDir,
        imageBuildSteps: agentImageBuildSteps(harness, opts.agentCli),
        extraTimeoutSec: replayBudgetSec,
      });

      try {
        const replay = yield* replayFailedTrajectory(
          session,
          trajectory.right,
          replayTimeoutSec
        );
        yield* uploadInstruction(session, instruction);

        const run = yield* runAgentCli({
          session,
          harness,
          opts: opts.agentCli,
          instructionPath: REMOTE_INSTRUCTION,
          timeoutMs: meta.maxAgentTimeoutSec * 1000 + AGENT_TIMEOUT_MARGIN_MS,
        });

        const testResult = yield* runTerminalBenchVerifier(
          session,
          meta,
          tasksDir
        );

        const { reward } = testResult;
        const testOutput = run.failureDetail
          ? `${run.failureDetail}\n\n${testResult.output}`
          : testResult.output;
        const completion = run.finalText ?? run.rawStream;
        const messages: ModelMessage[] = [
          { role: MessageRole.User, content: instruction },
          ...run.assistantMessages,
        ];

        return {
          sample: {
            ...state.sample,
            metadata: {
              ...state.sample.metadata,
              reward,
              testOutput,
              messageMode: opts.messageMode,
              recoveryInstructionBytes: instructionBytes,
              summaryUsed: resolved.summaryUsed,
              summaryFellBack: resolved.summaryFellBack,
              summaryCost: resolved.summary?.usage?.totalCost ?? 0,
              summaryTimeMs: resolved.summary?.generationTimeMs ?? 0,
              priorModel: trajectory.right.modelName,
              priorMessages: trajectory.right.messages.length,
              replayAttempted: replay.attempted,
              replayFailed: replay.failed,
              replaySkippedInterrupted: replay.skippedInterrupted,
              replayTotalExecutable: replay.totalExecutable,
              ...agentCliMetadata(harness.id, run),
              ...agentNetworkDeviation(meta),
            },
          },
          messages,
          responseItems: run.responseItems,
          output: {
            completion,
            message: { role: MessageRole.Assistant, content: completion },
            usage: addUsage(run.usage, resolved.summary?.usage),
            generationTimeMs:
              (run.generationTimeMs ?? 0) +
              (resolved.summary?.generationTimeMs ?? 0),
          },
          completed: true,
        };
      } finally {
        yield* session.destroy();
      }
    });
}
