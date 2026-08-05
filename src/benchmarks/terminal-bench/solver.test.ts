import { describe, expect, it } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { gen, provide, runPromise } from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  mergeAll as layerMergeAll,
  provide as layerProvide,
} from "effect/Layer";

import {
  noopProgressLayer,
  noopCheckpointLayer,
} from "../../../test/helpers/noop-progress-layer";
import type { Sample, TaskState } from "../../harness/core";
import { initialTaskState, ScoreValue } from "../../harness/core";
import { Solver } from "../../harness/solver";
import { readTerminalBenchMeta } from "./dataset";
import { makeFakeSandboxLayer, SandboxSession } from "./sandbox";
import { terminalBenchScorer } from "./scorer";
import type { TerminalBenchSolverOpts } from "./solver";
import { parseModel, piSolver } from "./solver";
import { seedTasksDir } from "./tasks-source";

async function runPiSolver(
  sandboxLayer: Layer<SandboxSession>,
  opts: TerminalBenchSolverOpts = SOLVER_OPTS
): Promise<TaskState> {
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      return Solver.of(piSolver(sessionFactory, opts));
    })
  );
  return runPromise(
    gen(function* () {
      const solver = yield* Solver;
      return yield* solver(sampleState());
    }).pipe(
      provide(
        layerMergeAll(
          solverLayer.pipe(layerProvide(sandboxLayer)),
          noopProgressLayer,
          noopCheckpointLayer
        )
      )
    )
  );
}

const fakeTasksDir = makeFakeTasksDir();
seedTasksDir(fakeTasksDir);

const PI_EVENT_STREAM = [
  JSON.stringify({
    type: "message_update",
    assistantMessageEvent: { type: "text_delta", delta: "..." },
  }),
  JSON.stringify({
    type: "message_end",
    message: {
      role: "assistant",
      usage: {
        input: 1000,
        output: 500,
        cacheRead: 200,
        cacheWrite: 300,
        cost: { total: 0.01 },
      },
    },
  }),
].join("\n");

const SOLVER_OPTS: TerminalBenchSolverOpts = {
  model: "openrouter/anthropic/claude-sonnet-4",
  apiKey: "sk-test",
  thinking: "medium",
};

function sampleState(): ReturnType<typeof initialTaskState> {
  const sample: Sample = {
    id: "terminal_bench-adaptive-rejection-sampler",
    input: "implement an adaptive rejection sampler",
    target: { text: "adaptive-rejection-sampler" },
    metadata: {
      taskId: "adaptive-rejection-sampler",
      dockerImage: "alexgshaw/adaptive-rejection-sampler:20251031",
      maxAgentTimeoutSec: 900,
      maxTestTimeoutSec: 900,
      difficulty: "medium",
      category: "scientific-computing",
    },
  };
  return initialTaskState(sample);
}
describe("terminal-bench pi solver", () => {
  it("stashes reward=1 and scores Correct when tests pass", async () => {
    const layer = makeFakeSandboxLayer({
      reward: 1,
      testOutput: "1 passed",
      agentEventStream: PI_EVENT_STREAM,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    const meta = readTerminalBenchMeta(finalState.sample.metadata);
    expect(meta?.reward).toBe(1);
    expect(meta?.testOutput).toBe("1 passed");
    expect(finalState.completed).toBe(true);
    const score = await runPromise(
      terminalBenchScorer(finalState, finalState.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });
  it("stashes reward=0 and scores Incorrect when tests fail", async () => {
    const layer = makeFakeSandboxLayer({
      reward: 0,
      testOutput: "1 failed",
      agentEventStream: PI_EVENT_STREAM,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    const meta = readTerminalBenchMeta(finalState.sample.metadata);
    expect(meta?.reward).toBe(0);
    const score = await runPromise(
      terminalBenchScorer(finalState, finalState.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
  });
  it("parses usage from the pi event stream", async () => {
    const layer = makeFakeSandboxLayer({
      reward: 1,
      agentEventStream: PI_EVENT_STREAM,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    const output = finalState.output;
    if (output === undefined || output.usage === undefined) {
      throw new Error("solver returned no output/usage");
    }
    expect(output.usage.inputTokens).toBe(1500);
    expect(output.usage.outputTokens).toBe(500);
    expect(output.usage.totalTokens).toBe(2000);
    expect(output.usage.reasoningTokens).toBe(0);
    expect(output.usage.totalCost).toBe(0.01);
  });
  it("runs pi without an appended system prompt by default", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer);
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    expect(piCall.argv[2]).not.toContain("--append-system-prompt");
    expect(piCall.env).not.toHaveProperty("TB_APPEND_SYSTEM_PROMPT");
  });
  it("passes an appended system prompt to pi through the exec environment", async () => {
    const appendSystemPrompt = "Work like a caveman: keep it simple.";
    const execCalls: NonNullable<
      Parameters<typeof makeFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer, { ...SOLVER_OPTS, appendSystemPrompt });
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    expect(piCall.argv[2]).toContain(
      '--append-system-prompt "$TB_APPEND_SYSTEM_PROMPT"'
    );
    expect(piCall.env["TB_APPEND_SYSTEM_PROMPT"]).toBe(appendSystemPrompt);
  });
  it("does not write a pi models.json for non-openrouter providers", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer, {
      ...SOLVER_OPTS,
      model: "anthropic/claude-sonnet-4",
    });
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    expect(piCall.argv[2]).not.toContain("models.json");
    expect(piCall.env).not.toHaveProperty("TB_PI_MODELS_JSON");
  });
  it("normalizes bare OpenRouter router models and preserves other model forms", () => {
    expect(parseModel("openrouter/auto-beta")).toEqual([
      "openrouter",
      "openrouter/auto-beta",
    ]);
    expect(parseModel("openrouter/openrouter/auto-beta")).toEqual([
      "openrouter",
      "openrouter/auto-beta",
    ]);
    expect(parseModel("openrouter/anthropic/claude-sonnet-4")).toEqual([
      "openrouter",
      "anthropic/claude-sonnet-4",
    ]);
    expect(parseModel("anthropic/claude-sonnet-4")).toEqual([
      "anthropic",
      "claude-sonnet-4",
    ]);
    expect(() => parseModel("auto-beta")).toThrow(
      'terminal-bench pi solver requires a model in "provider/model" form'
    );
  });
  it("writes a provider-level anthropic cache compat for concrete openrouter models", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer);
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    expect(piCall.argv[2]).toContain(
      "printf '%s' \"$TB_PI_MODELS_JSON\" > ~/.pi/agent/models.json"
    );
    const modelsJson = piCall.env["TB_PI_MODELS_JSON"];
    if (modelsJson === undefined) {
      throw new Error("TB_PI_MODELS_JSON missing from the pi exec environment");
    }
    const parsed: unknown = JSON.parse(modelsJson);
    expect(parsed).toEqual({
      providers: {
        openrouter: {
          compat: {
            thinkingFormat: "openrouter",
            cacheControlFormat: "anthropic",
          },
        },
      },
    });
  });
  it("normalizes a bare OpenRouter router model before invoking pi", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer, {
      ...SOLVER_OPTS,
      model: "openrouter/auto-beta",
      sessionId: "workflow-123",
    });
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    expect(piCall.env).toMatchObject({
      TB_PROVIDER: "openrouter",
      TB_MODEL: "openrouter/auto-beta",
    });
    const modelsJson = piCall.env["TB_PI_MODELS_JSON"];
    if (modelsJson === undefined) {
      throw new Error("TB_PI_MODELS_JSON missing from the pi exec environment");
    }
    const parsed: unknown = JSON.parse(modelsJson);
    expect(parsed).toMatchObject({
      providers: {
        openrouter: {
          headers: { "x-session-id": "workflow-123" },
          models: [{ id: "openrouter/auto-beta" }],
        },
      },
    });
  });
  it("writes a pi models.json into the agent dir for openrouter router models", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer, {
      ...SOLVER_OPTS,
      model: "openrouter/openrouter/phaser",
    });
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    expect(piCall.argv[2]).toContain(
      "printf '%s' \"$TB_PI_MODELS_JSON\" > ~/.pi/agent/models.json"
    );
    const modelsJson = piCall.env["TB_PI_MODELS_JSON"];
    if (modelsJson === undefined) {
      throw new Error("TB_PI_MODELS_JSON missing from the pi exec environment");
    }
    const parsed: unknown = JSON.parse(modelsJson);
    expect(parsed).toMatchObject({
      providers: {
        openrouter: {
          models: [
            {
              id: "openrouter/phaser",
              compat: {
                thinkingFormat: "openrouter",
                cacheControlFormat: "anthropic",
              },
              cost: { input: 2, output: 10, cacheRead: 0.2, cacheWrite: 2.5 },
            },
          ],
        },
      },
    });
  });
  it("writes a session header for an unknown preset model", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer, {
      ...SOLVER_OPTS,
      model: "openrouter/@preset/advisor-terra-sol",
      sessionId: "workflow-123",
    });
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    const modelsJson = piCall.env["TB_PI_MODELS_JSON"];
    if (modelsJson === undefined) {
      throw new Error("TB_PI_MODELS_JSON missing from the pi exec environment");
    }
    const parsed: unknown = JSON.parse(modelsJson);
    expect(parsed).toEqual({
      providers: {
        openrouter: {
          headers: { "x-session-id": "workflow-123" },
          compat: {
            thinkingFormat: "openrouter",
            cacheControlFormat: "anthropic",
          },
        },
      },
    });
  });
});

function makeFakeTasksDir(): string {
  const dir = join(
    tmpdir(),
    `terminal-bench-test-${Math.random().toString(36).slice(2)}`
  );
  const taskDir = join(dir, "adaptive-rejection-sampler");
  const testsDir = join(taskDir, "tests");
  mkdirSync(testsDir, { recursive: true });
  writeFileSync(
    join(taskDir, "task.toml"),
    [
      'schema_version = "1.1"',
      "[task]",
      'name = "terminal-bench/adaptive-rejection-sampler"',
      'description = "test"',
      "[metadata]",
      'author_name = "test"',
      'author_email = "test@test"',
      'difficulty = "medium"',
      'category = "scientific-computing"',
      "[agent]",
      "timeout_sec = 900.0",
      "[verifier]",
      "timeout_sec = 900.0",
      "[environment]",
      'docker_image = "test:latest"',
      "cpus = 1",
      "memory_mb = 2048",
      "gpus = 0",
    ].join("\n")
  );
  writeFileSync(
    join(taskDir, "instruction.md"),
    "implement an adaptive rejection sampler"
  );
  writeFileSync(
    join(testsDir, "test.sh"),
    "#!/bin/bash\necho 1 > /logs/verifier/reward.txt"
  );
  return dir;
}
