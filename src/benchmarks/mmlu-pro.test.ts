import { describe, expect, it } from "bun:test";

import { ScoreValue } from "../harness/core";
import type { RunResult } from "../harness/run";
import { MMLU_PRO_BENCHMARK, mmluProRecordToSample } from "./mmlu-pro";
import type { MmluProCotExamplesByCategory } from "./mmlu-pro-prompt";

const RECORD = {
  question: "Which answer is correct?",
  options: ["first", "N/A", "third"],
  answer: "B",
  category: "business",
  src: "test",
} as const;

const EXAMPLES: MmluProCotExamplesByCategory = new Map([
  [
    "business",
    [
      {
        question: "Example question",
        options: ["example A", "N/A", "example C"],
        cotContent: "A: Let us reason. The answer is (C).",
      },
    ],
  ],
]);

function runResultWith(
  scores: readonly {
    category: string;
    value: ScoreValue;
  }[]
): RunResult {
  return {
    metrics: {
      accuracy:
        scores.filter((score) => score.value === ScoreValue.Correct).length /
        scores.length,
      totalQuestions: scores.length,
      correctAnswers: scores.filter(
        (score) => score.value === ScoreValue.Correct
      ).length,
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
      sampleId: `mmlu_pro-${index}`,
      epoch: 0,
      metadata: { category: score.category },
      score: { value: score.value, answer: null, explanation: "" },
    })),
  };
}
describe("mmluProRecordToSample", () => {
  it("builds the canonical few-shot prompt and preserves target metadata", () => {
    const sample = mmluProRecordToSample(RECORD, 4, EXAMPLES);
    expect(sample.input).toBe(
      'The following are multiple choice questions (with answers) about business. Think step by step and then output the answer in the format of "The answer is (X)" at the end.\n\n' +
        "Question: Example question\n" +
        "Options: A. example A\n" +
        "B. example C\n" +
        "Answer: Let us reason. The answer is (C).\n\n" +
        "Question: Which answer is correct?\n" +
        "Options: A. first\n" +
        "B. third\n" +
        "Answer: Let's think step by step.\n\n"
    );
    expect(sample.id).toBe("mmlu_pro-4");
    expect(sample.target.text).toBe("B");
    expect(sample.metadata).toEqual({ category: "business", src: "test" });
  });
  it("supports the full A–J option range", () => {
    const sample = mmluProRecordToSample(
      {
        ...RECORD,
        options: ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"],
        answer: "J",
      },
      0,
      EXAMPLES
    );
    expect(sample.input).toContain("J. 9");
    expect(sample.target.text).toBe("J");
  });
  it("rejects malformed record fields and out-of-range answers", () => {
    expect(() =>
      mmluProRecordToSample({ ...RECORD, question: 1 }, 0, EXAMPLES)
    ).toThrow(TypeError);
    expect(() =>
      mmluProRecordToSample({ ...RECORD, options: [] }, 0, EXAMPLES)
    ).toThrow(TypeError);
    expect(() =>
      mmluProRecordToSample({ ...RECORD, answer: "C" }, 0, EXAMPLES)
    ).toThrow(TypeError);
    expect(() =>
      mmluProRecordToSample({ ...RECORD, category: 1 }, 0, EXAMPLES)
    ).toThrow(TypeError);
    expect(() =>
      mmluProRecordToSample({ ...RECORD, src: 1 }, 0, EXAMPLES)
    ).toThrow(TypeError);
  });
});
describe("MMLU-Pro runLevelScores", () => {
  it("aggregates overall and per-category accuracy", () => {
    const scores = MMLU_PRO_BENCHMARK.runLevelScores?.(
      runResultWith([
        { category: "business", value: ScoreValue.Correct },
        { category: "business", value: ScoreValue.Incorrect },
        { category: "law", value: ScoreValue.Correct },
      ])
    );
    expect(scores).toEqual([
      {
        name: "mmlu_pro",
        metrics: { accuracy: { value: 2 / 3 }, total_questions: { value: 3 } },
      },
      {
        name: "mmlu_pro_business",
        metrics: { accuracy: { value: 0.5 }, total_questions: { value: 2 } },
      },
      {
        name: "mmlu_pro_law",
        metrics: { accuracy: { value: 1 }, total_questions: { value: 1 } },
      },
    ]);
  });
  it("uses mean-of-means accuracy and evaluated epoch counts across epochs", () => {
    const scores = MMLU_PRO_BENCHMARK.runLevelScores?.({
      metrics: {
        accuracy: 0.75,
        totalQuestions: 4,
        correctAnswers: 3,
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
      sampleScores: [
        {
          sampleId: "business-1",
          epoch: 0,
          metadata: { category: "business" },
          score: { value: ScoreValue.Correct, answer: null, explanation: "" },
        },
        {
          sampleId: "business-1",
          epoch: 1,
          metadata: { category: "business" },
          score: { value: ScoreValue.Incorrect, answer: null, explanation: "" },
        },
        {
          sampleId: "business-2",
          epoch: 0,
          metadata: { category: "business" },
          score: { value: ScoreValue.Correct, answer: null, explanation: "" },
        },
        {
          sampleId: "business-2",
          epoch: 1,
          metadata: { category: "business" },
          score: { value: ScoreValue.Correct, answer: null, explanation: "" },
        },
      ],
    });
    expect(scores?.[1]).toEqual({
      name: "mmlu_pro_business",
      metrics: { accuracy: { value: 0.75 }, total_questions: { value: 2 } },
    });
  });
});
