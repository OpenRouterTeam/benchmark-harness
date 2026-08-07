import { describe, expect, it } from "bun:test";

import { provide, runPromise, succeed } from "effect/Effect";
import { mergeAll } from "effect/Layer";

import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../../../test/helpers/noop-progress-layer";
import type { Sample, TaskState } from "../../../harness/core";
import { initialTaskState, ScoreValue } from "../../../harness/core";
import type { SampleScore } from "../../../harness/metric";
import type { RunResult } from "../../../harness/run";
import { assertRight } from "../../../internal/testing";
import { parseSchema } from "../../../internal/zod";
import type {
  ResponsesResult,
  ResponsesService,
} from "../../../providers/responses-client";
import type { SearchLaneConfig } from "../core/config";
import { SearchLaneConfigSchema } from "../core/config";
import {
  calculateDsqaGrade,
  dsqaPrimaryScore,
  dsqaRunLevelScores,
  dsqaScorer,
  makeDsqaSolver,
} from "./benchmark";

const SAMPLE: Sample = {
  id: "dsqa-0",
  input: "Name both countries.",
  target: { text: "Belgium, France" },
  metadata: { prompt_type: "Set Answer", problem_category: "Geography" },
};

const VERDICT = {
  explanation: "Both expected countries were found.",
  correctness_details: { Belgium: true, France: true },
  excessive_answers: [],
};

function rawVerdict(verdict = VERDICT): string {
  return JSON.stringify({
    "Answer Correctness": {
      Explanation: verdict.explanation,
      "Correctness Details": verdict.correctness_details,
      "Excessive Answers": verdict.excessive_answers,
    },
  });
}

function makeLane(): SearchLaneConfig {
  const result = parseSchema(SearchLaneConfigSchema, { engine: "exa" });
  assertRight(result);
  return result.right;
}

function fixtureResult(text: string, isJudge = false): ResponsesResult {
  return {
    id: "r",
    model: "m",
    status: "completed",
    output: [],
    usage: isJudge
      ? { inputTokens: 4, outputTokens: 2, totalTokens: 6, cost: 0.001 }
      : { inputTokens: 10, outputTokens: 5, totalTokens: 15, cost: 0.02 },
    text,
    generationId: "g",
    provider: null,
    generationTimeMs: 0,
  };
}

function stateWithVerdict(verdict: unknown): TaskState {
  const state = initialTaskState(SAMPLE);
  return {
    ...state,
    sample: {
      ...state.sample,
      metadata: { ...state.sample.metadata, verdict },
    },
    output: {
      completion: "Belgium and France",
      message: { role: "assistant", content: "Belgium and France" },
    },
    completed: true,
  };
}
describe("DSQA benchmark", () => {
  it("scores complete answers without excess as correct", async () => {
    const score = await runPromise(
      dsqaScorer(stateWithVerdict(VERDICT), SAMPLE.target)
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });
  it("scores missing expected or excessive answers as incorrect", async () => {
    const missing = await runPromise(
      dsqaScorer(
        stateWithVerdict({
          ...VERDICT,
          correctness_details: { Belgium: true, France: false },
        }),
        SAMPLE.target
      )
    );
    const excessive = await runPromise(
      dsqaScorer(
        stateWithVerdict({ ...VERDICT, excessive_answers: ["Italy"] }),
        SAMPLE.target
      )
    );
    expect(missing.value).toBe(ScoreValue.Incorrect);
    expect(excessive.value).toBe(ScoreValue.Incorrect);
  });
  it("runs one search generation and one judge call", async () => {
    let calls = 0;
    const service: ResponsesService = {
      send: (body) => {
        calls += 1;
        const isJudge = body.model === "google/gemini-2.5-flash";
        return succeed(
          fixtureResult(isJudge ? rawVerdict() : "Belgium and France", isJudge)
        );
      },
    };
    const solver = makeDsqaSolver(service, {
      model: "m",
      instructions: "research it",
      lane: makeLane(),
    });
    const state = await runPromise(
      solver(initialTaskState(SAMPLE)).pipe(
        provide(mergeAll(noopProgressLayer, noopCheckpointLayer))
      )
    );
    expect(calls).toBe(2);
    expect(state.output?.usage?.totalCost).toBeCloseTo(0.021, 5);
    expect(state.sample.metadata?.["verdict"]).toEqual(VERDICT);
    expect((await runPromise(dsqaScorer(state, SAMPLE.target))).value).toBe(
      ScoreValue.Correct
    );
  });
  it("skips judging an empty generated answer", async () => {
    let calls = 0;
    const service: ResponsesService = {
      send: () => {
        calls += 1;
        return succeed(fixtureResult(""));
      },
    };
    const solver = makeDsqaSolver(service, {
      model: "m",
      instructions: "research it",
      lane: makeLane(),
      retry: { maxRetries: 0 },
    });
    await expect(
      runPromise(
        solver(initialTaskState(SAMPLE)).pipe(
          provide(mergeAll(noopProgressLayer, noopCheckpointLayer))
        )
      )
    ).rejects.toThrow("search response had no answer text");
    expect(calls).toBe(1);
  });
});

describe("DSQA metrics", () => {
  it.each([
    [{ A: true, B: true }, [], 1, 1, 1, 1],
    [{ A: true, B: false }, [], 1, 0.5, 2 / 3, 0],
    [{ A: true, B: true }, ["C"], 2 / 3, 1, 0.8, 0],
    [{ A: true, B: false }, ["C"], 0.5, 0.5, 0.5, 0],
    [{ A: false, B: false }, [], 0, 0, 0, 0],
    [{}, [], 0, 0, 0, 0],
  ] satisfies [
    Record<string, boolean>,
    string[],
    number,
    number,
    number,
    number,
  ][])(
    "calculates paper metrics for %#",
    (correctnessDetails, excessiveAnswers, precision, recall, f1, correct) => {
      const grade = calculateDsqaGrade({
        explanation: "grade",
        correctness_details: correctnessDetails,
        excessive_answers: excessiveAnswers,
      });
      expect(grade.metrics.precision).toBeCloseTo(precision);
      expect(grade.metrics.recall).toBeCloseTo(recall);
      expect(grade.metrics.f1_score).toBeCloseTo(f1);
      expect(grade.metrics.fully_correct).toBe(correct);
      expect(
        Object.values(grade.metrics)
          .slice(3)
          .reduce((sum, value) => sum + value, 0)
      ).toBe(1);
    }
  );

  it("counts duplicate excessive answers as separate false positives", () => {
    const grade = calculateDsqaGrade({
      explanation: "grade",
      correctness_details: { A: true },
      excessive_answers: ["B", "B"],
    });
    expect(grade.metrics.precision).toBeCloseTo(1 / 3);
    expect(grade.metrics.recall).toBe(1);
    expect(grade.metrics.f1_score).toBeCloseTo(0.5);
  });

  it("macro-averages question metrics across epochs and uses F1 as primary", () => {
    const score = (
      sampleId: string,
      epoch: number,
      f1Verdict: typeof VERDICT
    ) =>
      ({
        sampleId,
        epoch,
        score: {
          value: ScoreValue.Incorrect,
          answer: null,
          explanation: "",
          trajectory: {
            kind: "judge_runs",
            runs: [calculateDsqaGrade(f1Verdict)],
          },
        },
      }) satisfies SampleScore;
    const sampleScores = [
      score("a", 0, VERDICT),
      score("a", 1, {
        ...VERDICT,
        correctness_details: { Belgium: true, France: false },
      }),
      score("b", 0, {
        ...VERDICT,
        correctness_details: { Belgium: false, France: false },
      }),
    ];
    const run: RunResult = {
      metrics: {
        accuracy: 0,
        totalQuestions: 2,
        correctAnswers: 0,
        skippedQuestions: 0,
      },
      usage: {
        inputTokens: 0,
        outputTokens: 0,
        totalTokens: 0,
        reasoningTokens: 0,
        totalCost: 0,
        generationTimeMs: 0,
      },
      sampleScores,
    };
    expect(dsqaRunLevelScores(run)[0]?.metrics["f1_score"]?.value).toBeCloseTo(
      5 / 12
    );
    expect(dsqaRunLevelScores(run)[0]?.metrics["samples_judged"]?.value).toBe(
      2
    );
    expect(dsqaPrimaryScore(run)?.value).toBeCloseTo(5 / 12);
    expect(dsqaPrimaryScore(run)?.weight).toBe(2);
  });
});
