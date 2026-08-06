import { describe, expect, it } from "bun:test";

import type { ResponsesRequest } from "@openrouter/sdk/models";
import {
  fail as effectFail,
  provide,
  runPromise,
  succeed as effectSucceed,
  suspend,
} from "effect/Effect";
import { mergeAll } from "effect/Layer";

import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../../../test/helpers/noop-progress-layer";
import type {
  Sample,
  ScorerTrajectory,
  TaskState,
} from "../../../harness/core";
import { initialTaskState, ScoreValue } from "../../../harness/core";
import type { SampleScore } from "../../../harness/metric";
import type { RunResult } from "../../../harness/run";
import { assertRight } from "../../../internal/testing";
import { parseSchema } from "../../../internal/zod";
import type {
  ResponsesResult,
  ResponsesSendOptions,
  ResponsesService,
} from "../../../providers/responses-client";
import { ResponsesError } from "../../../providers/responses-client";
import type { SearchLaneConfig } from "../core/config";
import { SearchLaneConfigSchema } from "../core/config";
import { hleRunLevelScores, hleScorer, makeHleSolver } from "./benchmark";

const SAMPLE: Sample = {
  id: "upstream-hle-id",
  input: "Q?",
  target: { text: "42" },
};

function stateWithVerdict(
  verdict: unknown,
  completion = "Exact Answer: 42"
): TaskState {
  const base = initialTaskState(SAMPLE);
  return {
    ...base,
    sample: { ...base.sample, metadata: { verdict } },
    output: {
      completion,
      message: { role: "assistant", content: completion },
    },
    completed: true,
  };
}

function makeLane(): SearchLaneConfig {
  const result = parseSchema(SearchLaneConfigSchema, { engine: "exa" });
  assertRight(result);
  return result.right;
}

function fixtureResult(
  text: string,
  usage: Readonly<Record<string, unknown>> | null = null
): ResponsesResult {
  return {
    id: "r",
    model: "m",
    status: "completed",
    output: [],
    usage,
    text,
    generationId: "g",
    provider: null,
    generationTimeMs: 0,
  };
}
describe("hleScorer", () => {
  it("scores a valid yes verdict as correct", async () => {
    const score = await runPromise(
      hleScorer(
        stateWithVerdict({
          extracted_final_answer: "42",
          reasoning: "matches",
          correct: "yes",
          confidence: 90,
          strict: true,
        }),
        SAMPLE.target
      )
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("42");
  });
  it("scores a malformed verdict as incorrect", async () => {
    const score = await runPromise(
      hleScorer(stateWithVerdict({ correct: "maybe" }), SAMPLE.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("failed validation");
  });
  it("scores an empty answer without a verdict as incorrect", async () => {
    const score = await runPromise(
      hleScorer(stateWithVerdict(undefined, ""), SAMPLE.target)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("no verdict");
  });
});
describe("makeHleSolver", () => {
  it("runs generation then judging and merges their usage", async () => {
    const requests: ResponsesRequest[] = [];
    const options: ResponsesSendOptions[] = [];
    const service: ResponsesService = {
      send: (body, sendOptions) => {
        requests.push(body);
        options.push(sendOptions);
        if (body.text === undefined) {
          return effectSucceed(
            fixtureResult("Exact Answer: 42", {
              inputTokens: 10,
              outputTokens: 5,
              totalTokens: 15,
              outputTokensDetails: { reasoningTokens: 2 },
              cost: 0.02,
              serverToolUseDetails: {
                webSearchRequests: 3,
                toolCallsRequested: 4,
                toolCallsExecuted: 3,
              },
            })
          );
        }
        return effectSucceed(
          fixtureResult(
            JSON.stringify({
              extracted_final_answer: "42",
              reasoning: "matches",
              correct: "yes",
              confidence: 91,
              strict: true,
            }),
            {
              inputTokens: 4,
              outputTokens: 2,
              totalTokens: 6,
              outputTokensDetails: { reasoningTokens: 1 },
              cost: 0.001,
            }
          )
        );
      },
    };
    const solver = makeHleSolver(service, {
      model: "m",
      instructions: "research it",
      lane: makeLane(),
      versionOverride: "worker-version",
    });
    const state = await runPromise(
      solver(initialTaskState(SAMPLE)).pipe(
        provide(mergeAll(noopProgressLayer, noopCheckpointLayer))
      )
    );
    expect(requests).toHaveLength(2);
    expect(options.map((option) => option.versionOverride)).toEqual([
      "worker-version",
      "worker-version",
    ]);
    expect(state.output?.usage).toEqual({
      inputTokens: 14,
      outputTokens: 7,
      totalTokens: 21,
      reasoningTokens: 3,
      totalCost: 0.021,
      serverToolUse: {
        webSearchRequests: 3,
        toolCallsRequested: 4,
        toolCallsExecuted: 3,
      },
    });
    const score = await runPromise(hleScorer(state, SAMPLE.target));
    expect(score.value).toBe(ScoreValue.Correct);
  });
  it("retries a transient judge failure without rerunning generation", async () => {
    let generationAttempts = 0;
    let judgeAttempts = 0;
    const service: ResponsesService = {
      send: (body) =>
        suspend(() => {
          if (body.text === undefined) {
            generationAttempts += 1;
            return effectSucceed(fixtureResult("Exact Answer: 42"));
          }
          judgeAttempts += 1;
          return judgeAttempts === 1
            ? effectFail(
                new ResponsesError({ message: "transient", retryable: true })
              )
            : effectSucceed(
                fixtureResult(
                  JSON.stringify({
                    extracted_final_answer: "42",
                    reasoning: "matches",
                    correct: "yes",
                    confidence: 80,
                    strict: true,
                  })
                )
              );
        }),
    };
    const solver = makeHleSolver(service, {
      model: "m",
      instructions: "research it",
      lane: makeLane(),
      retry: { maxRetries: 2, baseDelayMs: 1 },
    });
    await runPromise(
      solver(initialTaskState(SAMPLE)).pipe(
        provide(mergeAll(noopProgressLayer, noopCheckpointLayer))
      )
    );
    expect(generationAttempts).toBe(1);
    expect(judgeAttempts).toBe(2);
  });
  it("skips judging an empty generation", async () => {
    let calls = 0;
    const service: ResponsesService = {
      send: () => {
        calls += 1;
        return effectSucceed(fixtureResult(""));
      },
    };
    const solver = makeHleSolver(service, {
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

function verdictTrajectory(confidence: number): ScorerTrajectory {
  return {
    kind: "judge_runs",
    runs: [
      {
        extracted_final_answer: "a",
        reasoning: "r",
        correct: "yes",
        confidence,
        strict: true,
      },
    ],
  };
}

function sampleScore(trajectory: ScorerTrajectory | undefined): SampleScore {
  return {
    sampleId: "s",
    epoch: 0,
    score: {
      value: ScoreValue.Correct,
      answer: "a",
      explanation: "e",
      ...(trajectory !== undefined && { trajectory }),
    },
  };
}

function runResult(sampleScores: SampleScore[]): RunResult {
  return {
    metrics: {
      accuracy: 0,
      totalQuestions: sampleScores.length,
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
}
describe("hleRunLevelScores", () => {
  it("averages confidence over valid judged samples only", () => {
    const scores = hleRunLevelScores(
      runResult([
        sampleScore(verdictTrajectory(90)),
        sampleScore({ kind: "judge_runs", runs: [{ malformed: true }] }),
        sampleScore(verdictTrajectory(50)),
      ])
    );
    expect(scores).toEqual([
      {
        name: "hle",
        metrics: {
          mean_stated_confidence: { value: 70 },
          samples_judged: { value: 2 },
        },
      },
    ]);
  });
  it("returns no confidence metrics when no sample was judged", () => {
    expect(hleRunLevelScores(runResult([sampleScore(undefined)]))).toEqual([]);
  });
});
