import { sync } from "effect/Effect";

import { ScoreValue } from "../../../harness/core";
import type { ScorerService } from "../../../harness/scorer";
import { mmluProExtractAnswer } from "./mmlu-pro-extract";

export const mmluProScorer: ScorerService = (state, target) =>
  sync(() => {
    const extracted = state.output
      ? mmluProExtractAnswer(state.output.completion)
      : null;
    const targetAnswer = target.text.trim().toUpperCase();
    const isCorrect = extracted !== null && extracted === targetAnswer;
    return {
      value: isCorrect ? ScoreValue.Correct : ScoreValue.Incorrect,
      answer: extracted,
      explanation: extracted
        ? `Extracted '${extracted}' from response, target was '${targetAnswer}'`
        : "No answer found; canonical MMLU-Pro random-guesses null answers, but this harness scores them as Incorrect deterministically.",
    };
  });
