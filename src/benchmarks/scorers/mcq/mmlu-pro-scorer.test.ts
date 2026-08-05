import { describe, expect, it } from "bun:test";

import { runPromise } from "effect/Effect";

import {
  initialTaskState,
  MessageRole,
  ScoreValue,
} from "../../../harness/core";
import { mmluProScorer } from "./mmlu-pro-scorer";
describe("mmluProScorer", () => {
  it("scores the canonical extracted answer against the target", async () => {
    const state = {
      ...initialTaskState({
        id: "sample",
        input: "question",
        target: { text: "C" },
      }),
      output: {
        completion: "The answer is (C).",
        message: { role: MessageRole.Assistant, content: "The answer is (C)." },
      },
    };
    const score = await runPromise(mmluProScorer(state, { text: "c" }));
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("C");
  });
  it("scores a missing canonical answer as deterministic incorrect", async () => {
    const state = initialTaskState({
      id: "sample",
      input: "question",
      target: { text: "C" },
    });
    const score = await runPromise(mmluProScorer(state, { text: "C" }));
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.answer).toBeNull();
  });
});
