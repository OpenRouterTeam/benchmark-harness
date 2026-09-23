import { afterAll, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { failureOption } from "effect/Cause";
import {
  fail,
  gen,
  provide,
  runPromise,
  runPromiseExit,
  succeed,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  mergeAll as layerMergeAll,
  provide as layerProvide,
} from "effect/Layer";
import { getOrThrow } from "effect/Option";

import { assertFailure } from "../../../test/helpers/exit-asserts";
import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../../test/helpers/noop-progress-layer";
import type { FakeTerminalBenchBehavior } from "../../../test/helpers/terminal-bench-sandbox";
import {
  makeTerminalBenchFakeSandboxLayer,
  SandboxSession,
} from "../../../test/helpers/terminal-bench-sandbox";
import type { ModelMessage, Sample, TaskState } from "../../harness/core";
import {
  initialTaskState,
  MessageRole,
  ScoreValue,
  SolverError,
} from "../../harness/core";
import { Solver } from "../../harness/solver";
import { getOriHarness } from "../agent-cli/harness";
import { terminalBenchScorer } from "../terminal-bench/scorer";
import { REMOTE_INSTRUCTION } from "../terminal-bench/session";
import type { RecoveryBenchSampleMeta } from "./dataset";
import { readRecoveryBenchMeta } from "./dataset";
import {
  formatMessagesAsText,
  RECOVERY_PREAMBLE,
  SUMMARY_FALLBACK,
} from "./prompts";
import type { RecoveryBenchSolverOpts, Summarizer } from "./solver";
import { recoveryBenchSolver, resolveMessageContext } from "./solver";
import { resetCheckoutCache, seedTasksRoot } from "./tasks-source";
import { sha256Hex, verifyTrajectoryBytes } from "./trajectory-source";

const GENERATION_ID = "gen-1786484980-H6OpVHdz7070QlmacXWO";

const CLAUDE_STREAM = [
  JSON.stringify({ type: "system", subtype: "init", session_id: "s-1" }),
  JSON.stringify({
    type: "assistant",
    message: {
      id: GENERATION_ID,
      role: "assistant",
      model: "deepseek/deepseek-v4",
      content: [{ type: "text", text: "Trying a different approach." }],
    },
  }),
  JSON.stringify({
    type: "result",
    subtype: "success",
    is_error: false,
    result: "Recovered.",
    duration_ms: 1500,
    total_cost_usd: 0.01,
    api_error_status: null,
    usage: { input_tokens: 100, output_tokens: 20 },
  }),
].join("\n");

const TRAJECTORY = JSON.stringify({
  schema_version: "ATIF-v1.6",
  agent: { name: "terminus-2", model_name: "claude-haiku-4-5" },
  steps: [
    { step_id: 0, source: "user", message: "fix the build" },
    {
      step_id: 1,
      source: "agent",
      message: "Looking around.",
      tool_calls: [
        {
          tool_call_id: "a",
          function_name: "terminus_2_command",
          arguments: { keystrokes: "ls /app\n", duration: 0.5 },
        },
        {
          tool_call_id: "b",
          function_name: "terminus_2_command",
          arguments: { keystrokes: "make broken\n", duration: 2 },
        },
        {
          tool_call_id: "c",
          function_name: "terminus_2_command",
          arguments: { keystrokes: "python serve.py\n", duration: 5 },
        },
        {
          tool_call_id: "d",
          function_name: "terminus_2_command",
          arguments: { keystrokes: "C-c", duration: 0.1 },
        },
      ],
    },
  ],
});

const PRIOR_MESSAGES: readonly ModelMessage[] = [
  { role: MessageRole.User, content: "fix the build" },
  { role: MessageRole.Assistant, content: "Looking around." },
];

const META: RecoveryBenchSampleMeta = {
  taskId: "fix-build",
  trialDir: "01234567-fix-build__abc",
  trajectoryUrl:
    "https://media.githubusercontent.com/media/letta-ai/recovery-bench/x/trajectory.json",
  trajectorySha256: sha256Hex(TRAJECTORY),
  trajectoryBytes: Buffer.byteLength(TRAJECTORY, "utf8"),
  recoveryBenchCommit: "c".repeat(40),
  terminalBenchCommit: "d".repeat(40),
  dockerImage: "example/fix-build:20251031",
  maxAgentTimeoutSec: 900,
  maxTestTimeoutSec: 600,
  difficulty: "medium",
  category: "software-engineering",
  cpus: 2,
  memoryMb: 4096,
  gpus: 0,
  allowInternet: false,
};

const TASK_INSTRUCTION = "Make the build pass.";

function makeFakeTasksDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "recovery-bench-solver-test-"));
  const taskDir = join(dir, "fix-build");
  mkdirSync(join(taskDir, "tests"), { recursive: true });
  writeFileSync(join(taskDir, "task.toml"), 'version = "1.0"\n');
  writeFileSync(join(taskDir, "instruction.md"), `${TASK_INSTRUCTION}\n`);
  writeFileSync(join(taskDir, "tests", "test.sh"), "exit 0\n");
  return dir;
}

const fakeTasksDir = makeFakeTasksDir();
seedTasksRoot(fakeTasksDir);
afterAll(() => {
  resetCheckoutCache();
  rmSync(fakeTasksDir, { recursive: true, force: true });
});

function sampleState(): TaskState {
  const sample: Sample = {
    id: "recovery_bench-01234567-fix-build__abc",
    input: TASK_INSTRUCTION,
    target: { text: "fix-build" },
    metadata: META,
  };
  return initialTaskState(sample);
}

function baseOpts(
  overrides: Partial<RecoveryBenchSolverOpts> = {}
): RecoveryBenchSolverOpts {
  return {
    agentCli: {
      model: "deepseek/deepseek-v4",
      apiKey: "sk-test",
      agentReasoningEffort: "medium",
    },
    messageMode: "full",
    maxInstructionBytes: 64000,
    fetchTrajectory: () => succeed(TRAJECTORY),
    ...overrides,
  };
}

function solverLayer(
  sandboxLayer: Layer<SandboxSession>,
  opts: RecoveryBenchSolverOpts
) {
  return layerEffect(Solver)(
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      return Solver.of(
        recoveryBenchSolver(sessionFactory, opts, getOriHarness("claude"))
      );
    })
  ).pipe(layerProvide(sandboxLayer));
}

function runSolver(
  sandboxLayer: Layer<SandboxSession>,
  opts: RecoveryBenchSolverOpts = baseOpts()
): Promise<TaskState> {
  return runPromise(
    gen(function* () {
      const solver = yield* Solver;
      return yield* solver(sampleState());
    }).pipe(
      provide(
        layerMergeAll(
          solverLayer(sandboxLayer, opts),
          noopProgressLayer,
          noopCheckpointLayer
        )
      )
    )
  );
}

function runSolverExit(
  sandboxLayer: Layer<SandboxSession>,
  opts: RecoveryBenchSolverOpts
) {
  return runPromiseExit(
    gen(function* () {
      const solver = yield* Solver;
      return yield* solver(sampleState());
    }).pipe(
      provide(
        layerMergeAll(
          solverLayer(sandboxLayer, opts),
          noopProgressLayer,
          noopCheckpointLayer
        )
      )
    )
  );
}

function replayCommands(
  execCalls: NonNullable<FakeTerminalBenchBehavior["execCalls"]>
): readonly (readonly [string, number | undefined])[] {
  return execCalls
    .filter((c) => c.argv[0] === "bash" && c.argv[1] === "-lc")
    .map((c) => [c.argv[2] ?? "", c.timeoutMs] as const);
}

describe("recovery-bench solver", () => {
  it("replays the failed trajectory, uploads the recovery instruction and scores the verifier", async () => {
    const execCalls: NonNullable<FakeTerminalBenchBehavior["execCalls"]> = [];
    const uploadedFiles: NonNullable<
      FakeTerminalBenchBehavior["uploadedFiles"]
    > = [];
    const creates: NonNullable<FakeTerminalBenchBehavior["creates"]> = [];
    let destroyed = 0;
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      testOutput: "1 passed",
      agentEventStream: CLAUDE_STREAM,
      execCalls,
      uploadedFiles,
      creates,
      onDestroy: () => {
        destroyed += 1;
      },
      execOverride: (argv) =>
        argv[2] === "make broken"
          ? { stdout: "", stderr: "make: *** error", exitCode: 2 }
          : undefined,
    });

    const finalState = await runSolver(layer);

    expect(creates.length).toBe(1);
    expect(creates[0]?.imageTag).toBe(META.dockerImage);
    expect(creates[0]?.cpus).toBe(META.cpus);
    expect(creates[0]?.memoryMb).toBe(META.memoryMb);
    expect(creates[0]?.timeoutSec).toBe(900 + 600 + 2 * 15 + 300);
    expect(creates[0]?.allowInternet).toBe(true);
    expect(replayCommands(execCalls)).toEqual([
      ["ls /app", 15000],
      ["make broken", 15000],
    ]);
    const instruction = uploadedFiles.find(
      (f) => f.remotePath === REMOTE_INSTRUCTION
    );
    expect(instruction?.content).toBe(
      `${RECOVERY_PREAMBLE}\n\n--- PREVIOUS ATTEMPT CONTEXT ---\n${formatMessagesAsText(PRIOR_MESSAGES)}\n\n--- ORIGINAL TASK ---\n${TASK_INSTRUCTION}`
    );
    const replayIndex = execCalls.findIndex((c) => c.argv[2] === "ls /app");
    const agentIndex = execCalls.findIndex((c) =>
      c.argv.join(" ").includes("ori claude")
    );
    expect(replayIndex).toBeGreaterThanOrEqual(0);
    expect(agentIndex).toBeGreaterThan(replayIndex);

    const meta = finalState.sample.metadata as Record<string, unknown>;
    expect(meta.reward).toBe(1);
    expect(meta.testOutput).toBe("1 passed");
    expect(meta.messageMode).toBe("full");
    expect(meta.priorModel).toBe("claude-haiku-4-5");
    expect(meta.priorMessages).toBe(2);
    expect(meta.replayAttempted).toBe(2);
    expect(meta.replayFailed).toBe(1);
    expect(meta.replaySkippedInterrupted).toBe(1);
    expect(meta.replayTotalExecutable).toBe(3);
    expect(meta.summaryUsed).toBe(false);
    expect(meta.summaryCost).toBe(0);
    expect(readRecoveryBenchMeta(finalState.sample.metadata)).toEqual(META);

    expect(finalState.completed).toBe(true);
    expect(finalState.output?.completion).toBe("Recovered.");
    expect(finalState.output?.usage.totalCost).toBe(0.01);
    expect(finalState.output?.generationTimeMs).toBe(1500);
    expect(finalState.messages[0]).toEqual({
      role: MessageRole.User,
      content: instruction?.content ?? "",
    });
    expect(destroyed).toBe(1);

    const score = await runPromise(
      terminalBenchScorer(finalState, finalState.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("uses the configured replay timeout and scores Incorrect on reward 0", async () => {
    const execCalls: NonNullable<FakeTerminalBenchBehavior["execCalls"]> = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 0,
      testOutput: "1 failed",
      agentEventStream: CLAUDE_STREAM,
      execCalls,
    });
    const finalState = await runSolver(
      layer,
      baseOpts({ replayCommandTimeoutSec: 3 })
    );
    expect(replayCommands(execCalls).map(([, t]) => t)).toEqual([3000, 3000]);
    const score = await runPromise(
      terminalBenchScorer(finalState, finalState.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
  });

  it("omits prior context in none mode and falls back to the fixed summary without a summarizer", async () => {
    const noneUploads: NonNullable<FakeTerminalBenchBehavior["uploadedFiles"]> =
      [];
    await runSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        agentEventStream: CLAUDE_STREAM,
        uploadedFiles: noneUploads,
      }),
      baseOpts({ messageMode: "none" })
    );
    expect(
      noneUploads.find((f) => f.remotePath === REMOTE_INSTRUCTION)?.content
    ).toBe(
      `${RECOVERY_PREAMBLE}\n\n--- ORIGINAL TASK ---\n${TASK_INSTRUCTION}`
    );

    const summaryUploads: NonNullable<
      FakeTerminalBenchBehavior["uploadedFiles"]
    > = [];
    const finalState = await runSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        agentEventStream: CLAUDE_STREAM,
        uploadedFiles: summaryUploads,
      }),
      baseOpts({ messageMode: "summary" })
    );
    expect(
      summaryUploads.find((f) => f.remotePath === REMOTE_INSTRUCTION)?.content
    ).toBe(
      `${RECOVERY_PREAMBLE}\n\n--- PREVIOUS ATTEMPT CONTEXT ---\n${SUMMARY_FALLBACK}\n\n--- ORIGINAL TASK ---\n${TASK_INSTRUCTION}`
    );
    const meta = finalState.sample.metadata as Record<string, unknown>;
    expect(meta.summaryUsed).toBe(true);
    expect(meta.summaryFellBack).toBe(true);
  });

  it("adds summarizer usage and latency to the sample totals", async () => {
    const summarize: Summarizer = () =>
      succeed({
        text: "The agent ran make and it failed.",
        usage: {
          inputTokens: 50,
          outputTokens: 10,
          totalTokens: 60,
          reasoningTokens: 0,
          totalCost: 0.002,
        },
        generationTimeMs: 400,
      });
    const uploadedFiles: NonNullable<
      FakeTerminalBenchBehavior["uploadedFiles"]
    > = [];
    const finalState = await runSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        agentEventStream: CLAUDE_STREAM,
        uploadedFiles,
      }),
      baseOpts({ messageMode: "summary", summarize })
    );
    expect(
      uploadedFiles.find((f) => f.remotePath === REMOTE_INSTRUCTION)?.content
    ).toContain(
      "--- PREVIOUS ATTEMPT CONTEXT ---\nThe agent ran make and it failed."
    );
    const meta = finalState.sample.metadata as Record<string, unknown>;
    expect(meta.summaryFellBack).toBe(false);
    expect(meta.summaryCost).toBe(0.002);
    expect(meta.summaryTimeMs).toBe(400);
    expect(finalState.output?.usage.totalCost).toBeCloseTo(0.012);
    expect(finalState.output?.usage.inputTokens).toBe(150);
    expect(finalState.output?.generationTimeMs).toBe(1900);
  });

  it("falls back to the fixed summary when the summarizer fails", async () => {
    const resolved = await runPromise(
      resolveMessageContext("summary", PRIOR_MESSAGES, () =>
        fail(new SolverError({ message: "model down" }))
      )
    );
    expect(resolved).toEqual({
      context: SUMMARY_FALLBACK,
      summaryUsed: true,
      summaryFellBack: true,
    });
  });

  it("fails before creating a sandbox when the trajectory is malformed or the instruction is too large", async () => {
    const creates: NonNullable<FakeTerminalBenchBehavior["creates"]> = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      agentEventStream: CLAUDE_STREAM,
      creates,
    });
    const malformed = await runSolverExit(
      layer,
      baseOpts({ fetchTrajectory: () => succeed("{not json") })
    );
    assertFailure(malformed);
    expect(getOrThrow(failureOption(malformed.cause)).message).toContain(
      "not valid JSON"
    );

    const tooLarge = await runSolverExit(
      layer,
      baseOpts({ maxInstructionBytes: 100 })
    );
    assertFailure(tooLarge);
    expect(getOrThrow(failureOption(tooLarge.cause)).message).toContain(
      "limit is 100"
    );
    expect(creates.length).toBe(0);
  });

  it("propagates trajectory fetch failures as solver errors", async () => {
    const exit = await runSolverExit(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        agentEventStream: CLAUDE_STREAM,
      }),
      baseOpts({
        fetchTrajectory: () =>
          fail(new SolverError({ message: "sha256 mismatch" })),
      })
    );
    assertFailure(exit);
    expect(getOrThrow(failureOption(exit.cause)).message).toBe(
      "sha256 mismatch"
    );
  });

  it("verifies trajectory bytes and digest against the manifest", () => {
    expect(verifyTrajectoryBytes(META, TRAJECTORY)).toBeUndefined();
    expect(verifyTrajectoryBytes(META, `${TRAJECTORY} `)).toContain("bytes");
    const sameLength = `${TRAJECTORY.slice(0, -1)}]`;
    expect(verifyTrajectoryBytes(META, sameLength)).toContain("sha256");
  });
});
