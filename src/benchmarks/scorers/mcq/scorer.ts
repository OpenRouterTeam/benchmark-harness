import { sync } from "effect/Effect";

import { ScoreValue } from "../../../harness/core";
import type { ScorerService } from "../../../harness/scorer";
import { extractMcqAnswer } from "./extract";

export const mcqScorer: ScorerService = (state, target) =>
  sync(() => {
    const extracted = state.output
      ? extractMcqAnswer(state.output.completion)
      : null;
    const targetAnswer = target.text.trim().toUpperCase();
    const isCorrect = extracted !== null && extracted === targetAnswer;
    return {
      value: isCorrect ? ScoreValue.Correct : ScoreValue.Incorrect,
      answer: extracted,
      explanation: extracted
        ? `Extracted '${extracted}' from response, target was '${targetAnswer}'`
        : "No answer found",
    };
  });
