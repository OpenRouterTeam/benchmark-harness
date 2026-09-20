import { sync } from "effect/Effect";

import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { parseSystemOneAnswers } from "../../providers/systemone-model";
import { extractMcqAnswer } from "../scorers/mcq/extract";
import { SYSTEMONE_QUESTION_ID } from "./prompt";
import type { ModelInterface } from "./schema";

export interface ChessExtraction {
  readonly letter: string | null;
  readonly probabilities?: Readonly<Record<string, number>>;
  readonly confidence?: number;
  readonly error?: string;
}

export function extractChatAnswer(completion: string): ChessExtraction {
  const letter = extractMcqAnswer(completion);
  return letter === null
    ? { letter, error: "no answer letter found" }
    : { letter };
}

export function extractSystemOneAnswer(completion: string): ChessExtraction {
  const answers = parseSystemOneAnswers(completion);
  if (Either.isLeft(answers)) {
    return { letter: null, error: answers.left };
  }
  const answer = answers.right[SYSTEMONE_QUESTION_ID];
  if (answer === undefined) {
    return { letter: null, error: `missing ${SYSTEMONE_QUESTION_ID} answer` };
  }
  if (answer.type !== "choice") {
    return { letter: null, error: `unexpected answer type ${answer.type}` };
  }
  return {
    letter: answer.choice.trim().toUpperCase(),
    ...(answer.probabilities !== undefined && {
      probabilities: answer.probabilities,
    }),
    ...(answer.confidence !== undefined && { confidence: answer.confidence }),
  };
}

export function extractChessAnswer(
  completion: string,
  modelInterface: ModelInterface
): ChessExtraction {
  switch (modelInterface) {
    case "chat": {
      return extractChatAnswer(completion);
    }
    case "systemone": {
      return extractSystemOneAnswer(completion);
    }
  }
}

export function makeChessScorer(modelInterface: ModelInterface): ScorerService {
  return (state, target) =>
    sync(() => {
      const targetLetter = target.text.trim().toUpperCase();
      const extraction =
        state.output === undefined
          ? { letter: null, error: "no model output" }
          : extractChessAnswer(state.output.completion, modelInterface);
      const isCorrect = extraction.letter === targetLetter;
      const pCorrect = extraction.probabilities?.[targetLetter];
      return {
        value: isCorrect ? ScoreValue.Correct : ScoreValue.Incorrect,
        answer: extraction.letter,
        explanation: JSON.stringify({
          target: targetLetter,
          extracted: extraction.letter,
          ...(extraction.error !== undefined && { error: extraction.error }),
          ...(pCorrect !== undefined && { pCorrect }),
          ...(extraction.probabilities !== undefined && {
            probabilities: extraction.probabilities,
          }),
          ...(extraction.confidence !== undefined && {
            confidence: extraction.confidence,
          }),
        }),
      };
    });
}
