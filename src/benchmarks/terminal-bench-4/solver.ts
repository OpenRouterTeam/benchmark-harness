import { gen, tryPromise } from "effect/Effect";

import type { ModelMessage, ModelUsage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import type { OriHarnessDef } from "../agent-cli/harness";
import type { AgentCliOpts } from "../agent-cli/runner";
import {
  agentCliMetadata,
  agentImageBuildSteps,
  runAgentCli,
} from "../agent-cli/runner";
import type { SandboxSessionFactory } from "../harbor/sandbox";
import { readTerminalBench4Meta } from "./dataset";
import { imageTags } from "./images";
import {
  agentNetworkDeviation,
  createAgentSession,
  createVerifierSession,
  REMOTE_INSTRUCTION,
  runCollectHooks,
  runVerifier,
  sandboxCollectHooks,
  transferArtifacts,
} from "./session";
import { ensureTasksCheckedOut, tasksDir } from "./tasks-source";

const AGENT_TIMEOUT_MARGIN_MS = 30_000;

const ZERO_USAGE: ModelUsage = {
  inputTokens: 0,
  outputTokens: 0,
  totalTokens: 0,
  reasoningTokens: 0,
  totalCost: 0,
};

export interface TerminalBench4SolverOpts extends AgentCliOpts {
  readonly imageRepo: string;
}

export function terminalBench4Solver(
  sessionFactory: SandboxSessionFactory,
  opts: TerminalBench4SolverOpts,
  harness: OriHarnessDef
): SolverService {
  return (state) =>
    gen(function* () {
      const meta = readTerminalBench4Meta(state.sample.metadata);
      if (meta === undefined) {
        return yield* new SolverError({
          message: `terminal-bench-4 solver received a sample without terminal-bench-4 metadata (id=${state.sample.id})`,
        });
      }
      const collectHooks = yield* sandboxCollectHooks(meta.collect);
      const root = yield* tryPromise({
        try: () => ensureTasksCheckedOut(),
        catch: (e: unknown) =>
          new SolverError({
            message: `Failed to check out terminal-bench-4 tasks: ${String(e)}`,
          }),
      });
      const images = imageTags(opts.imageRepo, meta.taskId);
      const agent = yield* createAgentSession({
        sessionFactory,
        meta,
        tasksDir: tasksDir(root),
        imageTag: images.agent,
        imageBuildSteps: agentImageBuildSteps(harness, opts),
      });
      try {
        const run = yield* runAgentCli({
          session: agent,
          harness,
          opts,
          instructionPath: REMOTE_INSTRUCTION,
          timeoutMs: meta.maxAgentTimeoutSec * 1000 + AGENT_TIMEOUT_MARGIN_MS,
        });
        yield* runCollectHooks(agent, collectHooks);
        const verifier = yield* createVerifierSession({
          sessionFactory,
          meta,
          imageTag: images.verifier,
        });
        try {
          yield* transferArtifacts({
            agent,
            verifier,
            artifacts: meta.artifacts,
          });
          yield* agent.destroy();
          const testResult = yield* runVerifier(verifier, meta);
          const testOutput = run.failureDetail
            ? `${run.failureDetail}\n\n${testResult.output}`
            : testResult.output;
          const completion = run.finalText ?? run.rawStream;
          const messages: ModelMessage[] = [
            { role: MessageRole.User, content: state.sample.input },
            ...run.assistantMessages,
          ];
          return {
            sample: {
              ...state.sample,
              metadata: {
                ...state.sample.metadata,
                reward: testResult.reward,
                testOutput,
                ...agentCliMetadata(harness.id, run),
                ...agentNetworkDeviation(meta),
              },
            },
            messages,
            responseItems: run.responseItems,
            output: {
              completion,
              message: { role: MessageRole.Assistant, content: completion },
              usage: run.usage ?? ZERO_USAGE,
              generationTimeMs: run.generationTimeMs ?? 0,
            },
            completed: true,
          };
        } finally {
          yield* verifier.destroy();
        }
      } finally {
        yield* agent.destroy();
      }
    });
}
