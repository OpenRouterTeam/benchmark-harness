import { describe, expect, it } from "bun:test";

import { fail, runPromise, succeed } from "effect/Effect";

import type { Score, TaskState } from "../../../harness/core";
import {
  initialTaskState,
  MessageRole,
  ScoreValue,
} from "../../../harness/core";
import type { RunResult } from "../../../harness/run";
import { DecisionsError } from "../../../judge/decisions";
import { mcqScorer } from "../mcq/scorer";
import type { ChoiceJudge } from "./integrity-rubric";
import {
  INTEGRITY_QUESTION,
  integrityRunLevelScores,
  withIntegrityRubric,
} from "./integrity-rubric";

const TARGET = { text: "B" };

function answered(completion: string): TaskState {
  return {
    ...initialTaskState({ id: "s1", input: "Which is it?", target: TARGET }),
    output: {
      completion,
      message: { role: MessageRole.Assistant, content: completion },
    },
  };
}

function judgeReturning(good: number): {
  readonly judge: ChoiceJudge;
  readonly calls: Readonly<Record<string, string>>[];
} {
  const calls: Readonly<Record<string, string>>[] = [];
  const judge: ChoiceJudge = (state) => {
    calls.push(state);
    return succeed({
      coherent_good: good,
      coherent_okay: 0,
      coherent_bad: 1 - good,
    });
  };
  return { judge, calls };
}

function verdictOf(score: Score): unknown {
  return score.trajectory?.kind === "judge_runs"
    ? score.trajectory.runs[0]
    : undefined;
}

function runOf(scores: readonly Score[]): RunResult {
  return {
    metrics: {
      accuracy: 0,
      totalQuestions: scores.length,
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
    sampleScores: scores.map((score, index) => ({
      sampleId: `s${index}`,
      epoch: 1,
      score,
    })),
  };
}

const PROSE = "The ester hydrolyses under base.\n\nAnswer: B";

describe("withIntegrityRubric", () => {
  it("records an intact verdict and keeps the answer score", async () => {
    const { judge, calls } = judgeReturning(0.9);
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(calls).toEqual([
      { user_request: "Which is it?", completion: PROSE },
    ]);
    expect(verdictOf(score)).toMatchObject({
      source: "judge",
      flagged: false,
    });
  });

  it("flags a corrupted completion without changing accuracy", async () => {
    const { judge } = judgeReturning(0.3);
    const base = await runPromise(mcqScorer(answered(PROSE), TARGET));
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe(base.answer);
    expect(score.explanation).toBe(base.explanation);
    expect(verdictOf(score)).toMatchObject({ source: "judge", flagged: true });
  });

  it("flags just above the threshold and not at it", async () => {
    const at = await runPromise(
      withIntegrityRubric(mcqScorer, judgeReturning(0.6).judge)(
        answered(PROSE),
        TARGET
      )
    );
    const above = await runPromise(
      withIntegrityRubric(mcqScorer, judgeReturning(0.59).judge)(
        answered(PROSE),
        TARGET
      )
    );
    expect(verdictOf(at)).toMatchObject({ flagged: false });
    expect(verdictOf(above)).toMatchObject({ flagged: true });
  });

  it("decides bare answers and empty output without the judge", async () => {
    const { judge, calls } = judgeReturning(0);
    const scorer = withIntegrityRubric(mcqScorer, judge);
    const bare = await runPromise(scorer(answered("Answer: B"), TARGET));
    const empty = await runPromise(scorer(answered("  "), TARGET));
    expect(calls).toEqual([]);
    expect(bare.value).toBe(ScoreValue.Correct);
    expect(verdictOf(bare)).toMatchObject({
      source: "bare_answer",
      flagged: false,
      notIntact: 0,
    });
    expect(verdictOf(empty)).toMatchObject({
      source: "empty",
      flagged: true,
      notIntact: 1,
    });
  });

  it("records a judge failure as an error, not a flag", async () => {
    const judge: ChoiceJudge = () =>
      fail(new DecisionsError("decisions HTTP 503"));
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(verdictOf(score)).toEqual({
      judge: "integrity",
      source: "error",
      error: "decisions HTTP 503",
    });
  });

  it("treats a verdict without coherent_good as an error", async () => {
    const judge: ChoiceJudge = () => succeed({});
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(verdictOf(score)).toMatchObject({ source: "error" });
  });

  it("asks the tuned integrity question", async () => {
    let asked: unknown;
    const judge: ChoiceJudge = (_state, question) => {
      asked = question;
      return succeed({ coherent_good: 1 });
    };
    await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(asked).toBe(INTEGRITY_QUESTION);
  });
});

describe("integrityRunLevelScores", () => {
  it("aggregates flagged rate and mean P(not intact) over scored samples", async () => {
    const scores = await Promise.all(
      [0.9, 0.2, 0.7].map((good) =>
        runPromise(
          withIntegrityRubric(mcqScorer, judgeReturning(good).judge)(
            answered(PROSE),
            TARGET
          )
        )
      )
    );
    const errored = await runPromise(
      withIntegrityRubric(mcqScorer, () => fail(new DecisionsError("boom")))(
        answered(PROSE),
        TARGET
      )
    );
    const [integrity] = integrityRunLevelScores(runOf([...scores, errored]));
    expect(integrity?.name).toBe("integrity");
    expect(integrity?.metrics["flagged_rate"]?.value).toBeCloseTo(1 / 3);
    expect(integrity?.metrics["mean_not_intact"]?.value).toBeCloseTo(
      (0.1 + 0.8 + 0.3) / 3
    );
    expect(integrity?.metrics["flagged"]?.value).toBe(1);
    expect(integrity?.metrics["scored"]?.value).toBe(3);
    expect(integrity?.metrics["errors"]?.value).toBe(1);
  });

  it("reports nothing when no sample carries an integrity verdict", async () => {
    const plain = await runPromise(mcqScorer(answered(PROSE), TARGET));
    expect(integrityRunLevelScores(runOf([plain]))).toEqual([]);
  });
});
