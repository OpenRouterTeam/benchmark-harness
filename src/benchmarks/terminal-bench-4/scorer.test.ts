import { describe, expect, it } from "bun:test";

import { runPromise } from "effect/Effect";

import type { Sample } from "../../harness/core";
import { initialTaskState, ScoreValue } from "../../harness/core";
import type { TerminalBench4SampleMeta } from "./dataset";
import { terminalBench4Scorer } from "./scorer";

const META: TerminalBench4SampleMeta = {
  taskId: "music-harmony",
  maxAgentTimeoutSec: 28_800,
  maxTestTimeoutSec: 600,
  category: "software-engineering",
  agentEnv: { cpus: 2, memoryMb: 4096, env: {}, allowInternet: true },
  verifierEnv: { cpus: 2, memoryMb: 4096, env: {}, allowInternet: false },
  artifacts: [],
  collect: [],
};

function sampleWith(reward: number | undefined, testOutput?: string): Sample {
  return {
    id: "terminal_bench_4-music-harmony",
    input: "harmonize",
    target: { text: "music-harmony" },
    metadata: {
      ...META,
      ...(reward !== undefined && { reward }),
      ...(testOutput !== undefined && { testOutput }),
    },
  };
}

describe("terminal-bench-4 scorer (pure)", () => {
  it("scores Correct when the stashed reward is 1", async () => {
    const state = initialTaskState(sampleWith(1, "1 passed"));
    const score = await runPromise(
      terminalBench4Scorer(state, state.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("music-harmony");
    expect(score.explanation).toBe("1 passed");
    expect(score.trajectory).toEqual({ kind: "verifier_log", log: "1 passed" });
  });
  it("scores Incorrect when the stashed reward is 0", async () => {
    const state = initialTaskState(sampleWith(0, "1 failed"));
    const score = await runPromise(
      terminalBench4Scorer(state, state.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
  });
  it("scores Incorrect when no reward is stashed", async () => {
    const state = initialTaskState(sampleWith(undefined));
    const score = await runPromise(
      terminalBench4Scorer(state, state.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toBe("");
    expect(score.trajectory).toBeUndefined();
  });
  it("scores Incorrect when metadata is not terminal-bench-4 metadata", async () => {
    const state = initialTaskState({
      ...sampleWith(1),
      metadata: { taskId: "music-harmony", reward: 1 },
    });
    const score = await runPromise(
      terminalBench4Scorer(state, state.sample.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
  });
});
