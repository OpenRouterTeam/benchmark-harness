import { describe, expect, it } from "bun:test";

import { runSync } from "effect/Effect";

import type { TaskState } from "../../harness/core";
import { MessageRole, ScoreValue } from "../../harness/core";
import { extractSystemOneAnswer, makeChessScorer } from "./scorer";

function makeState(completion: string | undefined): TaskState {
  return {
    sample: {
      id: "chess_move_ladder-1",
      input: "fen",
      target: { text: "B" },
    },
    messages: [],
    ...(completion !== undefined && {
      output: {
        completion,
        message: { role: MessageRole.Assistant, content: completion },
      },
    }),
    completed: true,
  };
}

const TARGET = { text: "B" };

const SYSTEMONE_COMPLETION = JSON.stringify({
  best_move: {
    type: "choice",
    choice: "B",
    probabilities: { A: 0.1, B: 0.7, C: 0.15, D: 0.05 },
    confidence: 0.64,
  },
});

describe("makeChessScorer(chat)", () => {
  const scorer = makeChessScorer("chat");
  it("extracts the trailing answer letter", () => {
    const score = runSync(
      scorer(makeState("Rxe7 wins the rook.\nAnswer: B"), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("B");
  });
  it("marks a wrong letter Incorrect", () => {
    const score = runSync(scorer(makeState("Answer: A"), TARGET));
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.answer).toBe("A");
  });
  it("records an extraction failure separately from a wrong answer", () => {
    const score = runSync(
      scorer(makeState("I think the rook capture is best."), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.answer).toBeNull();
    expect(score.explanation).toContain("no answer letter found");
  });
  it("handles a missing output", () => {
    const score = runSync(scorer(makeState(undefined), TARGET));
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("no model output");
  });
});

describe("makeChessScorer(systemone)", () => {
  const scorer = makeChessScorer("systemone");
  it("scores the typed choice and keeps the distribution", () => {
    const score = runSync(scorer(makeState(SYSTEMONE_COMPLETION), TARGET));
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("B");
    expect(score.explanation).toContain('"pCorrect":0.7');
    expect(score.explanation).toContain('"confidence":0.64');
  });
  it("rejects non-JSON completions", () => {
    const score = runSync(scorer(makeState("Answer: B"), TARGET));
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.answer).toBeNull();
  });
  it("rejects answers of the wrong question type", () => {
    const extraction = extractSystemOneAnswer(
      JSON.stringify({ best_move: { type: "noul", noul: 0.9 } })
    );
    expect(extraction.letter).toBeNull();
    expect(extraction.error).toContain("unexpected answer type");
  });
  it("rejects a response missing the question", () => {
    const extraction = extractSystemOneAnswer(
      JSON.stringify({ other: { type: "choice", choice: "B" } })
    );
    expect(extraction.letter).toBeNull();
    expect(extraction.error).toContain("missing best_move");
  });
});
