import { sync } from "effect/Effect";

import type { Score } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { RunResult } from "../../harness/run";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { parseSchema } from "../../internal/zod";
import type { BenchmarkPrimaryScore } from "../types";
import { readDecisionSampleMeta } from "./dataset";
import type { DecisionAction } from "./programs";
import {
  DECISION_ACTIONS,
  branchExpectations,
  isDecisionAction,
} from "./programs";
import type {
  DecisionSampleMeta,
  JudgeTrace,
  ProbablyRunMeta,
  ScoreDetail,
} from "./schema";
import { ProbablyRunMetaSchema, ScoreDetailSchema } from "./schema";

export function isEnactment(action: DecisionAction): boolean {
  return action !== "hold";
}

function readRunMeta(
  metadata: Readonly<Record<string, unknown>> | undefined
): ProbablyRunMeta | undefined {
  const parsed = parseSchema(ProbablyRunMetaSchema, metadata?.["probablyRun"]);
  return Either.isRight(parsed) ? parsed.right : undefined;
}

function meanTopProbability(meta: ProbablyRunMeta): number | null {
  if (meta.judges.length === 0) {
    return null;
  }
  const tops = meta.judges.map((judge) =>
    Math.max(0, ...Object.values(judge.probabilities))
  );
  return tops.reduce((acc, n) => acc + n, 0) / tops.length;
}

function judgeFor(
  meta: ProbablyRunMeta,
  label: string
): JudgeTrace | undefined {
  return meta.judges.find((judge) => judge.labels.includes(label));
}

export function branchAgreement(
  meta: ProbablyRunMeta,
  gold: DecisionAction
): number | null {
  const verdicts = branchExpectations(gold).flatMap((expectation) => {
    const judge = judgeFor(meta, expectation.label);
    if (judge === undefined) {
      return [];
    }
    const chosen = judge.chosen === expectation.label;
    return [expectation.expect === "chosen" ? chosen : !chosen];
  });
  return rate(verdicts) ?? null;
}

export function brierScore(
  meta: ProbablyRunMeta,
  gold: DecisionAction
): number | null {
  const scores = branchExpectations(gold).flatMap((expectation) => {
    const judge = judgeFor(meta, expectation.label);
    if (judge === undefined || expectation.expect !== "chosen") {
      return [];
    }
    const total = judge.labels
      .map(
        (label) =>
          (judge.probabilities[label] ?? 0) -
          (label === expectation.label ? 1 : 0)
      )
      .map((diff) => diff * diff)
      .reduce((acc, n) => acc + n, 0);
    return [total];
  });
  return mean(scores) ?? null;
}

function factRecall(
  meta: ProbablyRunMeta,
  facts: readonly string[]
): number | null {
  if (
    meta.mode !== "research" ||
    facts.length === 0 ||
    meta.researchDossier === null
  ) {
    return null;
  }
  const dossier = meta.researchDossier.toLowerCase();
  return (
    facts.filter((fact) => dossier.includes(fact.toLowerCase())).length /
    facts.length
  );
}

function evidenceCoverage(meta: ProbablyRunMeta): number | null {
  if (meta.mode !== "research" || meta.sectionsAvailable.length === 0) {
    return null;
  }
  const available = new Set(meta.sectionsAvailable);
  const read = meta.sectionsRead.filter((s) => available.has(s)).length;
  return read / meta.sectionsAvailable.length;
}

export function scoreDecision(
  meta: ProbablyRunMeta | undefined,
  goldText: string,
  sample: DecisionSampleMeta | undefined
): Score {
  if (!isDecisionAction(goldText)) {
    return {
      value: ScoreValue.Skipped,
      answer: null,
      explanation: JSON.stringify({
        failure: `Unknown gold action ${goldText}`,
      }),
    };
  }
  const predicted = meta?.action ?? null;
  const predictedAction =
    predicted !== null && isDecisionAction(predicted) ? predicted : null;
  const correct = predictedAction === goldText;
  const detail: ScoreDetail = {
    mode: meta?.mode ?? "judgment",
    predicted,
    gold: goldText,
    correct,
    enactmentAgreement:
      predictedAction !== null &&
      isEnactment(predictedAction) === isEnactment(goldText),
    judgeCount: meta?.judges.length ?? 0,
    meanTopProbability: meta === undefined ? null : meanTopProbability(meta),
    branchAgreement:
      meta === undefined ? null : branchAgreement(meta, goldText),
    brier: meta === undefined ? null : brierScore(meta, goldText),
    evidenceCoverage: meta === undefined ? null : evidenceCoverage(meta),
    factRecall:
      meta === undefined ? null : factRecall(meta, sample?.goldFacts ?? []),
    failure:
      meta?.failure ?? (meta === undefined ? "Run metadata missing" : null),
  };
  return {
    value: correct ? ScoreValue.Correct : ScoreValue.Incorrect,
    answer: predicted,
    explanation: JSON.stringify(detail),
  };
}

export const decisionScorer: ScorerService = (state, target) =>
  sync(() =>
    scoreDecision(
      readRunMeta(state.sample.metadata),
      target.text,
      readDecisionSampleMeta(state.sample.metadata)
    )
  );

function readDetails(result: RunResult): readonly ScoreDetail[] {
  return result.sampleScores.flatMap((sample) => {
    const json = Either.try((): unknown =>
      JSON.parse(sample.score.explanation)
    );
    if (Either.isLeft(json)) {
      return [];
    }
    const parsed = parseSchema(ScoreDetailSchema, json.right);
    return Either.isRight(parsed) ? [parsed.right] : [];
  });
}

function mean(values: readonly number[]): number | undefined {
  return values.length === 0
    ? undefined
    : values.reduce((a, b) => a + b, 0) / values.length;
}

function meanDefined(values: readonly (number | null)[]): number | undefined {
  return mean(values.flatMap((v) => (v === null ? [] : [v])));
}

function rate(values: readonly boolean[]): number | undefined {
  return values.length === 0
    ? undefined
    : values.filter(Boolean).length / values.length;
}

export function decisionRunLevelScores(result: RunResult): readonly {
  readonly name: string;
  readonly metrics: Readonly<Record<string, { readonly value: number }>>;
}[] {
  const details = readDetails(result);
  if (details.length === 0) {
    return [];
  }
  const perAction = DECISION_ACTIONS.flatMap((action) => {
    const recall = rate(
      details.filter((d) => d.gold === action).map((d) => d.correct)
    );
    return recall === undefined ? [] : [[`recall_${action}`, recall] as const];
  });
  const macroRecall = mean(perAction.map(([, v]) => v));
  const entries: readonly (readonly [string, number | undefined])[] = [
    ["action_accuracy", rate(details.map((d) => d.correct))],
    ["enactment_agreement", rate(details.map((d) => d.enactmentAgreement))],
    ["macro_recall", macroRecall],
    [
      "hold_precision",
      rate(details.filter((d) => d.predicted === "hold").map((d) => d.correct)),
    ],
    ["run_failure_rate", rate(details.map((d) => d.failure !== null))],
    [
      "mean_top_probability",
      meanDefined(details.map((d) => d.meanTopProbability)),
    ],
    ["branch_agreement", meanDefined(details.map((d) => d.branchAgreement))],
    ["brier", meanDefined(details.map((d) => d.brier))],
    ["evidence_coverage", meanDefined(details.map((d) => d.evidenceCoverage))],
    ["fact_recall", meanDefined(details.map((d) => d.factRecall))],
    ...perAction,
  ];
  return [
    {
      name: "probably_decisions",
      metrics: Object.fromEntries(
        entries.flatMap(([name, value]) =>
          value === undefined ? [] : [[name, { value }]]
        )
      ),
    },
  ];
}

export function decisionPrimaryScore(
  result: RunResult
): BenchmarkPrimaryScore | undefined {
  if (result.sampleScores.length === 0) {
    return undefined;
  }
  const metrics = decisionRunLevelScores(result)[0]?.metrics;
  return {
    value: metrics?.["action_accuracy"]?.value ?? 0,
    weight: result.sampleScores.length,
  };
}
