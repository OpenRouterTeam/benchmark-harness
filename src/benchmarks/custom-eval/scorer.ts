import { sync } from "effect/Effect";

/**
 * Deterministic scorer dispatch for declarative custom evals. Pure functions:
 * every verdict is reproducible from the stored completion + spec, which is
 * what lets rescoring reuse trajectories with zero model calls later.
 */
import type { Score } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { extractMcqAnswer } from "../scorers/mcq/extract";
import type { EvalScorer } from "./spec";

const score = (
  isCorrect: boolean,
  answer: string | null,
  explanation: string
): Score => ({
  value: isCorrect ? ScoreValue.Correct : ScoreValue.Incorrect,
  answer,
  explanation,
});

/** Last number in the text (handles commas and signs), or null. */
export function extractLastNumber(text: string): number | null {
  const matches = text.replaceAll(",", "").match(/-?\d+(?:\.\d+)?/g);
  if (!matches || matches.length === 0) {
    return null;
  }
  const last = Number(matches.at(-1));
  return Number.isFinite(last) ? last : null;
}

export function scoreCompletion(
  scorer: EvalScorer,
  completion: string,
  target: string
): Score {
  switch (scorer.kind) {
    case "exact": {
      const normalize = (value: string): string => {
        const trimmed = scorer.trim ? value.trim() : value;
        return scorer.caseSensitive ? trimmed : trimmed.toLowerCase();
      };
      const ok = normalize(completion) === normalize(target);
      return score(
        ok,
        completion.trim(),
        ok ? "exact match" : `expected "${target}"`
      );
    }
    case "contains": {
      const haystack = scorer.caseSensitive
        ? completion
        : completion.toLowerCase();
      const needle = scorer.caseSensitive ? target : target.toLowerCase();
      const ok = haystack.includes(needle);
      return score(
        ok,
        null,
        ok ? `completion contains "${target}"` : `missing "${target}"`
      );
    }
    case "regex": {
      const flags = scorer.caseSensitive ? "u" : "iu";
      const ok = new RegExp(scorer.pattern, flags).test(completion);
      return score(
        ok,
        null,
        ok ? `matched /${scorer.pattern}/` : `no match for /${scorer.pattern}/`
      );
    }
    case "choice": {
      const extracted = extractMcqAnswer(completion);
      const expected = target.trim().toUpperCase();
      const ok = extracted !== null && extracted === expected;
      return score(
        ok,
        extracted,
        extracted
          ? `extracted '${extracted}', target '${expected}'`
          : "no answer letter found"
      );
    }
    case "numeric": {
      const actual = extractLastNumber(completion);
      const expected = Number(target.replaceAll(",", ""));
      if (actual === null || !Number.isFinite(expected)) {
        return score(
          false,
          actual === null ? null : String(actual),
          "no comparable number found"
        );
      }
      const absOk = Math.abs(actual - expected) <= scorer.absoluteTolerance;
      const relOk =
        scorer.relativeTolerance > 0 &&
        Math.abs(actual - expected) <=
          Math.abs(expected) * scorer.relativeTolerance;
      const ok = actual === expected || absOk || relOk;
      return score(
        ok,
        String(actual),
        ok ? "within tolerance" : `expected ${expected}, got ${actual}`
      );
    }
    default: {
      scorer satisfies never;
      return score(false, null, "unknown scorer");
    }
  }
}

export function makeCustomEvalScorer(scorer: EvalScorer): ScorerService {
  return (state, target) =>
    sync(() =>
      scoreCompletion(scorer, state.output?.completion ?? "", target.text)
    );
}
