import { describe, expect, it } from "bun:test";

import { runPromise } from "effect/Effect";

import { initialTaskState, ScoreValue } from "../../harness/core";
import type { Criterion, DracoPanelConfig, JudgeRun } from "./schemas";
import { dracoScorer } from "./scorer-draco";

const config: DracoPanelConfig = {
  name: "test",
  description: "",
  type: "single",
  model: "openai/gpt-4o-mini",
  fallbackModel: undefined,
  provider: undefined,
  synthesisModel: undefined,
  analysisModels: [],
  tools: undefined,
  searchEngine: "exa",
  blockedDomains: [],
  versionOverride: undefined,
  judgeModel: "google/gemini-3.1-pro-preview",
  judgeRuns: 1,
  judgeTemperature: undefined,
  judgeReasoningEffort: "low",
  criterionConcurrency: 10,
  timeout: 1800,
  concurrency: 5,
};

const criteria: Criterion[] = [
  {
    id: "a",
    section: "Factual",
    sectionId: "factual",
    weight: 10,
    requirement: "States X",
  },
  {
    id: "b",
    section: "Factual",
    sectionId: "factual",
    weight: 8,
    requirement: "Covers Y",
  },
];

function runVerdict(
  verdicts: {
    id: string;
    verdict: "MET" | "UNMET";
  }[]
): JudgeRun[] {
  return [
    {
      runNum: 1,
      status: "ok",
      judgeModel: config.judgeModel,
      cost: 0.01,
      latencyMs: 100,
      timestamp: "",
      verdicts: verdicts.map((v) => ({
        criterionId: v.id,
        section: "Factual",
        weight: criteria.find((c) => c.id === v.id)!.weight,
        verdict: v.verdict,
        justification: "",
      })),
    },
  ];
}

function stateFor(verdicts: JudgeRun[], completion = "the answer") {
  const sample = {
    id: "task-1",
    input: "Q?",
    target: { text: "" },
    metadata: { domain: "Tech", criteria, verdicts, generation: { cost: 0.5 } },
  };
  const state = initialTaskState(sample);
  return {
    ...state,
    output: {
      completion,
      message: { role: "assistant" as const, content: completion },
      generationTimeMs: 10,
    },
  };
}
describe("dracoScorer", () => {
  it("returns Correct (value C) when meanNormalized >= 50", async () => {
    const state = stateFor(
      runVerdict([
        { id: "a", verdict: "MET" },
        { id: "b", verdict: "MET" },
      ])
    );
    const score = await runPromise(dracoScorer(config)(state, { text: "" }));
    expect(score.value).toBe(ScoreValue.Correct);
    const explanation = JSON.parse(score.explanation);
    expect(explanation.normalized).toBe(100);
    expect(explanation.generationCost).toBe(0.5);
    expect(explanation.judgingCost).toBe(0.01);
    expect(explanation.totalCost).toBe(0.51);
  });
  it("returns Incorrect (value I) when meanNormalized < 50", async () => {
    const state = stateFor(
      runVerdict([
        { id: "a", verdict: "UNMET" },
        { id: "b", verdict: "UNMET" },
      ])
    );
    const score = await runPromise(dracoScorer(config)(state, { text: "" }));
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(JSON.parse(score.explanation).normalized).toBe(0);
  });
  it("carries the model completion as answer", async () => {
    const state = stateFor(
      runVerdict([{ id: "a", verdict: "MET" }]),
      "final text"
    );
    const score = await runPromise(dracoScorer(config)(state, { text: "" }));
    expect(score.answer).toBe("final text");
  });
  it("records the judge runs as the scorer trajectory", async () => {
    const runs = runVerdict([{ id: "a", verdict: "MET" }]);
    const state = stateFor(runs);
    const score = await runPromise(dracoScorer(config)(state, { text: "" }));
    expect(score.trajectory).toEqual({ kind: "judge_runs", runs });
  });
  it("omits the trajectory when no judge runs were recorded", async () => {
    const state = stateFor([]);
    const score = await runPromise(dracoScorer(config)(state, { text: "" }));
    expect(score.trajectory).toBeUndefined();
  });
});
