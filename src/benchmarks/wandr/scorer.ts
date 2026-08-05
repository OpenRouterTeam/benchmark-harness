import { succeed } from "effect/Effect";

import type { Score, Target, TaskState } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { RunResult } from "../../harness/run";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import type { BenchmarkPrimaryScore } from "../types";
import type { WandrRewards } from "./schema";
import {
  WANDR_REWARD_NAMES,
  WandrRewardsSchema,
  ZERO_WANDR_REWARDS,
} from "./schema";

export interface WandrScoreMeta {
  readonly rewards?: WandrRewards;
  readonly verifierOutput?: string;
}

export function readWandrScoreMeta(
  metadata?: Readonly<Record<string, unknown>>
): WandrScoreMeta | undefined {
  if (metadata === undefined) {
    return undefined;
  }
  const rewards = parseSchema(WandrRewardsSchema, metadata["rewards"]);
  const verifierOutput = metadata["verifierOutput"];
  return {
    ...(Either.isRight(rewards) && { rewards: rewards.right }),
    ...(typeof verifierOutput === "string" && { verifierOutput }),
  };
}

export const wandrScorer: ScorerService = (
  state: TaskState,
  target: Target
) => {
  const meta = readWandrScoreMeta(state.sample.metadata);
  const softF1 = meta?.rewards?.soft_f1_full ?? 0;
  const explanation = JSON.stringify({
    rewards: meta?.rewards ?? null,
    verifierOutput: meta?.verifierOutput ?? "",
  });
  const score: Score = {
    value: softF1 > 0 ? ScoreValue.Correct : ScoreValue.Incorrect,
    answer: target.text,
    explanation,
    trajectory: { kind: "verifier_log", log: meta?.verifierOutput ?? "" },
  };
  return succeed(score);
};

/**
 * Samples that were actually evaluated. Skipped scores are infrastructure
 * failures (degraded solver/model errors), excluded from the accuracy
 * denominator by `aggregateScores` — the reward mean and primary-score weight
 * must exclude them the same way.
 */
function scoredWandrSamples(result: RunResult): RunResult["sampleScores"] {
  return result.sampleScores.filter(
    (sample) => sample.score.value !== ScoreValue.Skipped
  );
}

export function wandrRunLevelScores(result: RunResult): readonly {
  name: string;
  metrics: Readonly<
    Record<
      string,
      {
        value: number;
      }
    >
  >;
}[] {
  const rewards = scoredWandrSamples(result).flatMap((sample) => {
    const parsed = Either.try((): unknown =>
      JSON.parse(sample.score.explanation)
    );
    if (Either.isLeft(parsed) || !isRecord(parsed.right)) {
      return [ZERO_WANDR_REWARDS];
    }
    const validated = parseSchema(WandrRewardsSchema, parsed.right["rewards"]);
    return Either.isRight(validated) ? [validated.right] : [ZERO_WANDR_REWARDS];
  });
  if (rewards.length === 0) {
    return [];
  }
  return [
    {
      name: "wandr",
      metrics: Object.fromEntries(
        WANDR_REWARD_NAMES.map((name) => [
          name,
          {
            value:
              rewards.reduce((sum, reward) => sum + reward[name], 0) /
              rewards.length,
          },
        ])
      ),
    },
  ];
}

export function wandrPrimaryScore(
  result: RunResult
): BenchmarkPrimaryScore | undefined {
  const weight = scoredWandrSamples(result).length;
  if (weight === 0) {
    return undefined;
  }
  const metrics = wandrRunLevelScores(result)[0]?.metrics;
  return {
    value: metrics?.["soft_f1_full"]?.value ?? 0,
    weight,
  };
}
