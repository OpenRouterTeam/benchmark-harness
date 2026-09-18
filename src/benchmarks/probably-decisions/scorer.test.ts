import { describe, expect, test } from "bun:test";

import { ScoreValue } from "../../harness/core";
import type { RunResult } from "../../harness/run";
import {
  COHERENT_LABEL,
  LEAKED_KEY_LABEL,
  LOAD_LABEL,
  REMEDY_LABELS,
  STATIC_ONLY_LABEL,
} from "./programs";
import type { JudgeTrace, ProbablyRunMeta } from "./schema";
import { ScoreDetailSchema } from "./schema";
import {
  branchAgreement,
  brierScore,
  decisionPrimaryScore,
  decisionRunLevelScores,
  scoreDecision,
} from "./scorer";

function judge(
  labels: readonly string[],
  chosen: string,
  p: number,
  line = 1
): JudgeTrace {
  const rest = (1 - p) / (labels.length - 1);
  return {
    line,
    labels: [...labels],
    probabilities: Object.fromEntries(
      labels.map((label) => [label, label === chosen ? p : rest])
    ),
    chosen,
    threshold: 0.8,
  };
}

const NO = "no";

function judgmentMeta(
  action: string | null,
  judges: readonly JudgeTrace[],
  failure: string | null = null
): ProbablyRunMeta {
  return {
    mode: "judgment",
    action,
    judges: [...judges],
    researchDossier: null,
    sectionsAvailable: [],
    sectionsRead: [],
    researchSteps: 0,
    failure,
  };
}

const FRONTIER_RUN = judgmentMeta("frontier_block", [
  judge([LEAKED_KEY_LABEL, NO], NO, 0.95, 1),
  judge([STATIC_ONLY_LABEL, NO], NO, 0.9, 2),
  judge([LOAD_LABEL, NO], NO, 0.85, 3),
  judge([COHERENT_LABEL, NO], NO, 0.9, 4),
  judge(
    [
      REMEDY_LABELS.frontier_block,
      REMEDY_LABELS.inference_block,
      REMEDY_LABELS.account_ban,
    ],
    REMEDY_LABELS.frontier_block,
    0.8,
    5
  ),
]);

function detailOf(explanation: string) {
  return ScoreDetailSchema.parse(JSON.parse(explanation));
}

describe("probably-decisions scorer", () => {
  test("matching final action scores Correct with full branch agreement", () => {
    const score = scoreDecision(FRONTIER_RUN, "frontier_block", undefined);
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("frontier_block");
    const detail = detailOf(score.explanation);
    expect(detail.branchAgreement).toBe(1);
    expect(detail.enactmentAgreement).toBe(true);
    expect(detail.judgeCount).toBe(5);
    expect(detail.failure).toBeNull();
  });

  test("wrong remedy is Incorrect but keeps enactment agreement", () => {
    const score = scoreDecision(FRONTIER_RUN, "account_ban", undefined);
    expect(score.value).toBe(ScoreValue.Incorrect);
    const detail = detailOf(score.explanation);
    expect(detail.enactmentAgreement).toBe(true);
    expect(detail.branchAgreement).toBe(0.8);
  });

  test("hold versus enactment disagrees on enactment", () => {
    const detail = detailOf(
      scoreDecision(FRONTIER_RUN, "hold", undefined).explanation
    );
    expect(detail.correct).toBe(false);
    expect(detail.enactmentAgreement).toBe(false);
    expect(detail.branchAgreement).toBeNull();
  });

  test("missing run metadata is Incorrect with a failure", () => {
    const score = scoreDecision(undefined, "hold", undefined);
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(detailOf(score.explanation).failure).toBe("Run metadata missing");
  });

  test("unknown gold action is Skipped", () => {
    expect(scoreDecision(FRONTIER_RUN, "shadow_ban", undefined).value).toBe(
      ScoreValue.Skipped
    );
  });

  test("branchAgreement counts only judged expectations", () => {
    const partial = judgmentMeta("throttle", [
      judge([LEAKED_KEY_LABEL, NO], LEAKED_KEY_LABEL, 0.9),
    ]);
    expect(branchAgreement(partial, "throttle")).toBe(0);
    expect(branchAgreement(partial, "key_revocation")).toBe(1);
    expect(
      branchAgreement(judgmentMeta("hold", []), "frontier_block")
    ).toBeNull();
  });

  test("brierScore is zero for a certain correct choice and rises with spread", () => {
    const certain = judgmentMeta("key_revocation", [
      judge([LEAKED_KEY_LABEL, NO], LEAKED_KEY_LABEL, 1),
    ]);
    expect(brierScore(certain, "key_revocation")).toBe(0);
    const unsure = judgmentMeta("key_revocation", [
      judge([LEAKED_KEY_LABEL, NO], LEAKED_KEY_LABEL, 0.6),
    ]);
    expect(brierScore(unsure, "key_revocation")).toBeCloseTo(0.32, 5);
    expect(brierScore(unsure, "hold")).toBeNull();
  });

  test("research mode reports fact recall and evidence coverage", () => {
    const meta: ProbablyRunMeta = {
      mode: "research",
      action: "hold",
      judges: [],
      researchDossier: "The accounts share JA4 T13D and one card fingerprint.",
      sectionsAvailable: ["signup", "funding", "traffic", "keys"],
      sectionsRead: ["signup", "funding", "signup"],
      researchSteps: 4,
      failure: null,
    };
    const detail = detailOf(
      scoreDecision(meta, "hold", {
        domain: "trust_and_safety",
        source: "sentinel",
        lead: "lead",
        dossier: "dossier",
        evidence: {},
        humanDecision: "denied",
        goldFacts: ["t13d", "card fingerprint", "declines"],
      }).explanation
    );
    expect(detail.correct).toBe(true);
    expect(detail.factRecall).toBeCloseTo(2 / 3, 5);
    expect(detail.evidenceCoverage).toBe(0.75);
  });

  test("run-level metrics aggregate per action and primary score is accuracy", () => {
    const sampleScores = [
      scoreDecision(FRONTIER_RUN, "frontier_block", undefined),
      scoreDecision(FRONTIER_RUN, "account_ban", undefined),
      scoreDecision(judgmentMeta("hold", [], "timeout"), "hold", undefined),
    ].map((score, i) => ({
      sample: { id: `s${i}`, input: "", target: { text: "" } },
      score,
    }));
    const result = { sampleScores } as unknown as RunResult;
    const metrics = decisionRunLevelScores(result)[0]?.metrics;
    expect(metrics?.["action_accuracy"]?.value).toBeCloseTo(2 / 3, 5);
    expect(metrics?.["recall_frontier_block"]?.value).toBe(1);
    expect(metrics?.["recall_account_ban"]?.value).toBe(0);
    expect(metrics?.["recall_hold"]?.value).toBe(1);
    expect(metrics?.["hold_precision"]?.value).toBe(1);
    expect(metrics?.["run_failure_rate"]?.value).toBeCloseTo(1 / 3, 5);
    expect(metrics?.["brier"]?.value).toBeGreaterThan(0);
    expect(metrics?.["fact_recall"]).toBeUndefined();
    expect(decisionPrimaryScore(result)).toEqual({
      value: 2 / 3,
      weight: 3,
    });
    expect(
      decisionPrimaryScore({ sampleScores: [] } as unknown as RunResult)
    ).toBeUndefined();
  });
});
