import { succeed } from "effect/Effect";
import * as Result from "effect/Result";

import type { Score, Target, TaskState } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { definedValues, isRecord } from "../../internal/guards";

export function parseReward(raw: string): number {
  const trimmed = raw.trim();
  const parsed = Result.try((): unknown => JSON.parse(trimmed));
  if (
    Result.isSuccess(parsed) &&
    isRecord(parsed.success) &&
    typeof parsed.success["reward"] === "number"
  ) {
    return parsed.success["reward"] >= 1 ? 1 : 0;
  }
  const value = Number.parseFloat(trimmed);
  return Number.isFinite(value) && value >= 1 ? 1 : 0;
}

export interface RewardMeta {
  readonly reward?: number;
  readonly verifierOutput?: string;
}

export function makeRewardScorer(
  readMeta: (
    metadata?: Readonly<Record<string, unknown>>
  ) => RewardMeta | undefined
): ScorerService {
  return (state: TaskState, target: Target) => {
    const meta = readMeta(state.sample.metadata);
    const reward = meta?.reward ?? 0;
    const score: Score = definedValues({
      value: reward >= 1 ? ScoreValue.Correct : ScoreValue.Incorrect,
      answer: target.text,
      explanation: meta?.verifierOutput ?? "",
      trajectory:
        meta?.verifierOutput !== undefined
          ? ({ kind: "verifier_log", log: meta.verifierOutput } as const)
          : undefined,
    });
    return succeed(score);
  };
}
