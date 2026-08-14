import { sync } from "effect/Effect";

import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";

const LETTERS = "abcdefghijklmnopqrstuvwxyz";

const ANSWER_LINE = /answer\s*[:-]\s*\**\s*\(?\s*([a-zA-Z])\b/i;

export function parseVgiAnswer(text: string, nOptions: number): string | null {
  if (!text) {
    return null;
  }
  const lineMatch = ANSWER_LINE.exec(text);
  if (lineMatch !== null) {
    const index = LETTERS.indexOf(lineMatch[1]!.toLowerCase());
    if (index !== -1 && index < nOptions) {
      return LETTERS[index]!.toUpperCase();
    }
  }
  for (const match of text.matchAll(/\b([a-zA-Z])\b/gu)) {
    const index = LETTERS.indexOf(match[1]!.toLowerCase());
    if (index !== -1 && index < nOptions) {
      return LETTERS[index]!.toUpperCase();
    }
  }
  return null;
}

export const vgiBenchScorer: ScorerService = (state, target) =>
  sync(() => {
    const completion = state.output?.completion ?? "";
    const rawN = state.sample.metadata?.["num_options"];
    const nOptions =
      typeof rawN === "number" && Number.isFinite(rawN) ? rawN : 0;
    const extracted = parseVgiAnswer(completion, nOptions);
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
