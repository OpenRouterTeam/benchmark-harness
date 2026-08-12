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
import {
  makeTerminalBenchFakeSandboxLayer,
  SandboxSession,
} from "../../../test/helpers/terminal-bench-sandbox";
import type { Sample, TaskState } from "../../harness/core";
import { initialTaskState, ScoreValue } from "../../harness/core";
import { Solver } from "../../harness/solver";
import {
  getCollectedGenerationIds,
  resetGenerationIds,
} from "../../runtime/generation-ids";
import { readTerminalBenchMeta } from "./dataset";
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

const PI_GENERATION_ID = "gen-1786486069-YL5yw4QMx5rf2VgScdIt";

const PI_STREAM_WITH_RESPONSE_ID = [
  JSON.stringify({
    type: "message_end",
    message: {
      role: "assistant",
      responseId: PI_GENERATION_ID,
      model: "anthropic/claude-haiku-4.5",
      provider: "openrouter",
      stopReason: "stop",
      errorMessage: null,
      usage: {
        input: 3761,
        output: 44,
        cacheRead: 0,
        cacheWrite: 0,
        reasoning: 31,
        totalTokens: 3805,
        cost: { total: 0.003981 },
      },
    },
  }),
].join("\n");

const PI_STREAM_API_ERROR = JSON.stringify({
  type: "message_end",
  message: {
    role: "assistant",
    stopReason: "error",
    errorMessage:
      '400: {"message":"anthropic/bogus is not a valid model ID","code":400}',
    content: [],
    usage: {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      cost: { total: 0 },
    },
  },
});

const SOLVER_OPTS: TerminalBenchSolverOpts = {
  model: "openrouter/anthropic/claude-sonnet-4",
  apiKey: "sk-test",
  thinking: "medium",
};

function sampleState(
  metadataOverrides?: Readonly<Record<string, unknown>>
): ReturnType<typeof initialTaskState> {
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
      ...metadataOverrides,
    },
  };
  return initialTaskState(sample);
}

async function runPiSolverWithSample(
  sandboxLayer: Layer<SandboxSession>,
  metadataOverrides: Readonly<Record<string, unknown>>
): Promise<TaskState> {
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      return Solver.of(piSolver(sessionFactory, SOLVER_OPTS));
    })
  );
  return runPromise(
    gen(function* () {
      const solver = yield* Solver;
      return yield* solver(sampleState(metadataOverrides));
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
describe("terminal-bench harbor sandbox convergence", () => {
  it("honors the resources and network policy the task declares", async () => {
    const creates: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["creates"]
    > = [];
    await runPiSolverWithSample(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        creates,
        agentExitCode: 0,
      }),
      { cpus: 4, memoryMb: 8192, allowInternet: false }
    );
    const create = creates[0];
    if (create === undefined) {
      throw new Error("fake sandbox did not capture the session creation");
    }
    expect(create.cpus).toBe(4);
    expect(create.memoryMb).toBe(8192);
    expect(create.allowInternet).toBe(false);
  });

  it("defaults resources when a task omits them", async () => {
    const creates: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["creates"]
    > = [];
    await runPiSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        creates,
        agentExitCode: 0,
      })
    );
    const create = creates[0];
    if (create === undefined) {
      throw new Error("fake sandbox did not capture the session creation");
    }
    expect(create.cpus).toBe(1);
    expect(create.memoryMb).toBe(2048);
    expect(create.allowInternet).toBe(true);
  });

  it("uploads the task tests and instruction into the sandbox", async () => {
    const creates: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["creates"]
    > = [];
    await runPiSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        creates,
        agentExitCode: 0,
      })
    );
    const uploads = creates[0]?.uploads ?? [];
    expect(uploads).toHaveLength(2);
    expect(uploads[0]).toMatchObject({ remotePath: "/tests", kind: "dir" });
    expect(uploads[1]).toMatchObject({
      remotePath: "/instruction.md",
      kind: "file",
    });
  });

  it("sizes the sandbox timeout to cover agent plus verifier", async () => {
    const creates: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["creates"]
    > = [];
    await runPiSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        creates,
        agentExitCode: 0,
      })
    );
    expect(creates[0]?.timeoutSec).toBe(900 + 900 + 300);
    expect(creates[0]?.workdir).toBe("/app");
  });

  it("reads a json reward document the way harbor does", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      testOutput: "ok",
      agentEventStream: PI_STREAM_WITH_RESPONSE_ID,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    expect(readTerminalBenchMeta(finalState.sample.metadata)?.reward).toBe(1);
  });
});

describe("terminal-bench pi solver", () => {
  it("stashes reward=1 and scores Correct when tests pass", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
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
    const layer = makeTerminalBenchFakeSandboxLayer({
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

  it("collects pi responseIds so the generation_ids column is populated", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      agentEventStream: PI_STREAM_WITH_RESPONSE_ID,
      agentExitCode: 0,
    });
    const ids = await runPromise(
      gen(function* () {
        yield* resetGenerationIds;
        const solver = yield* Solver;
        yield* solver(sampleState());
        return yield* getCollectedGenerationIds;
      }).pipe(
        provide(
          layerMergeAll(
            layerEffect(Solver)(
              gen(function* () {
                const sessionFactory = yield* SandboxSession;
                return Solver.of(piSolver(sessionFactory, SOLVER_OPTS));
              })
            ).pipe(layerProvide(layer)),
            noopProgressLayer,
            noopCheckpointLayer
          )
        )
      )
    );
    expect([...ids]).toEqual([PI_GENERATION_ID]);
  });

  it("parses pi reasoning tokens instead of reporting zero", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      agentEventStream: PI_STREAM_WITH_RESPONSE_ID,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    expect(finalState.output?.usage?.reasoningTokens).toBe(31);
    expect(finalState.output?.usage?.totalCost).toBe(0.003981);
  });

  it("surfaces a pi api error that would otherwise look like a zero-cost run", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 0,
      testOutput: "1 failed",
      agentEventStream: PI_STREAM_API_ERROR,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    const meta = readTerminalBenchMeta(finalState.sample.metadata);
    expect(meta?.testOutput).toContain("api errors");
    expect(meta?.testOutput).toContain("is not a valid model ID");
    expect(finalState.sample.metadata?.["agentIsError"]).toBe(true);
  });

  it("counts pi turns and tool calls from the event stream", async () => {
    const stream = [
      JSON.stringify({ type: "turn_start" }),
      JSON.stringify({ type: "tool_execution_start" }),
      JSON.stringify({ type: "tool_execution_end" }),
      JSON.stringify({ type: "tool_execution_end" }),
      JSON.stringify({ type: "turn_end" }),
      JSON.stringify({ type: "turn_end" }),
      PI_STREAM_WITH_RESPONSE_ID,
    ].join("\n");
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      agentEventStream: stream,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    expect(finalState.sample.metadata?.["agentTurns"]).toBe(2);
    expect(finalState.sample.metadata?.["agentToolCalls"]).toBe(2);
  });

  it("keeps pi stream events as responseItems", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      agentEventStream: PI_STREAM_WITH_RESPONSE_ID,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    expect(finalState.responseItems).toHaveLength(1);
    expect(finalState.responseItems?.[0]?.["type"]).toBe("message_end");
  });

  it("measures a wall-clock generationTimeMs for pi", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      agentEventStream: PI_STREAM_WITH_RESPONSE_ID,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    const elapsed = finalState.output?.generationTimeMs;
    expect(typeof elapsed).toBe("number");
    expect(elapsed).toBeGreaterThanOrEqual(0);
  });

  it("passes a pi system prompt override and tool lists through the environment", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer, {
      ...SOLVER_OPTS,
      systemPrompt: "You are terse.",
      allowedTools: ["bash", "edit"],
      disallowedTools: ["write"],
    });
    const piCall = execCalls[0];
    if (piCall === undefined) {
      throw new Error("fake sandbox did not capture the pi invocation");
    }
    expect(piCall.env["TB_SYSTEM_PROMPT"]).toBe("You are terse.");
    expect(piCall.env["TB_ALLOWED_TOOLS"]).toBe("bash,edit");
    expect(piCall.env["TB_DISALLOWED_TOOLS"]).toBe("write");
    expect(piCall.argv[2]).toContain('--system-prompt "$TB_SYSTEM_PROMPT"');
    expect(piCall.argv[2]).toContain('--tools "$TB_ALLOWED_TOOLS"');
    expect(piCall.argv[2]).toContain('--exclude-tools "$TB_DISALLOWED_TOOLS"');
  });

  it("applies pi config isolation flags only when requested", async () => {
    const withoutCalls: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    await runPiSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        execCalls: withoutCalls,
        agentExitCode: 0,
      })
    );
    expect(withoutCalls[0]?.argv[2]).not.toContain("--no-context-files");
    const withCalls: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    await runPiSolver(
      makeTerminalBenchFakeSandboxLayer({
        reward: 1,
        execCalls: withCalls,
        agentExitCode: 0,
      }),
      { ...SOLVER_OPTS, isolateAgentConfig: true }
    );
    const script = withCalls[0]?.argv[2] ?? "";
    expect(script).toContain("--no-extensions");
    expect(script).toContain("--no-skills");
    expect(script).toContain("--no-prompt-templates");
    expect(script).toContain("--no-context-files");
  });

  it("supports the max thinking level", async () => {
    const execCalls: NonNullable<
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      execCalls,
      agentExitCode: 0,
    });
    await runPiSolver(layer, { ...SOLVER_OPTS, thinking: "max" });
    expect(execCalls[0]?.argv[2]).toContain("--thinking max");
  });

  it("labels the pi path in metadata so harnesses are separable", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
      reward: 1,
      agentEventStream: PI_STREAM_WITH_RESPONSE_ID,
      agentExitCode: 0,
    });
    const finalState = await runPiSolver(layer);
    expect(finalState.sample.metadata?.["agent"]).toBe("pi");
    expect(finalState.sample.metadata?.["agentExitCode"]).toBe(0);
  });

  it("parses usage from the pi event stream", async () => {
    const layer = makeTerminalBenchFakeSandboxLayer({
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
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
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
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
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
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
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
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
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
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
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
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
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
      Parameters<typeof makeTerminalBenchFakeSandboxLayer>[0]["execCalls"]
    > = [];
    const layer = makeTerminalBenchFakeSandboxLayer({
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
