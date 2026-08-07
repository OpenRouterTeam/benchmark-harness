import { ScoreValue } from "../../harness/core";
import type { BenchmarkResultRow } from "../../results/parquet-schema";
import {
  qualityFromMetadata,
  runTotalsFromResultRows,
} from "./result-row-metrics";

export interface ArmTrial {
  readonly taskId: string;
  readonly epoch: number;
  readonly passed: boolean;
  readonly quality?: number;
  readonly subcheckScore?: number;
  readonly failureDetail?: string;
}

export interface ArmRun {
  readonly label: string;
  readonly trials: readonly ArmTrial[];
  readonly totalTokens: number;
  readonly totalCost: number;
}

export interface TaskComparison {
  readonly taskId: string;
  readonly baselinePassRate: number | undefined;
  readonly candidatePassRate: number | undefined;
  readonly baselineQuality: number | undefined;
  readonly candidateQuality: number | undefined;
  readonly delta: number | undefined;
  readonly candidateFailureDetail?: string;
}

export interface ArmComparison {
  readonly baselineLabel: string;
  readonly candidateLabel: string;
  readonly tasks: readonly TaskComparison[];
  readonly baselinePassRate: number;
  readonly candidatePassRate: number;
  readonly passRateDelta: number;
  readonly baselineQuality: number | undefined;
  readonly candidateQuality: number | undefined;
  readonly baselineSubcheckScore: number | undefined;
  readonly candidateSubcheckScore: number | undefined;
  readonly baselineTotalCost: number;
  readonly candidateTotalCost: number;
  readonly totalTokensDelta: number;
  readonly totalCostDelta: number;
}

export function armRunFromResultRows(
  label: string,
  rows: readonly BenchmarkResultRow[]
): ArmRun {
  const { totalCost, totalTokens } = runTotalsFromResultRows(rows);
  return {
    label,
    trials: rows
      .filter((row) => row.score_value !== ScoreValue.Skipped)
      .map((row) => {
        const { quality, subcheckScore } = qualityFromMetadata(row.metadata);
        return {
          taskId: row.sample_id,
          epoch: row.epoch,
          passed: row.score_value === ScoreValue.Correct,
          ...(quality !== undefined && { quality }),
          ...(subcheckScore !== undefined && { subcheckScore }),
          ...(row.score_value !== ScoreValue.Correct &&
            row.explanation !== null && { failureDetail: row.explanation }),
        };
      }),
    totalTokens,
    totalCost,
  };
}

export function compareArms(
  baseline: ArmRun,
  candidate: ArmRun
): ArmComparison {
  const baselineByTask = groupPassesByTask(baseline.trials);
  const candidateByTask = groupPassesByTask(candidate.trials);
  const taskIds = [
    ...new Set([...baselineByTask.keys(), ...candidateByTask.keys()]),
  ].toSorted();

  const tasks = taskIds.map((taskId): TaskComparison => {
    const baselinePassRate = passRate(baselineByTask.get(taskId));
    const candidatePassRate = passRate(candidateByTask.get(taskId));
    const candidateFailureDetail = candidate.trials.find(
      (t) => t.taskId === taskId && !t.passed && t.failureDetail !== undefined
    )?.failureDetail;
    return {
      taskId,
      baselinePassRate,
      candidatePassRate,
      baselineQuality: meanQuality(
        baseline.trials.filter((t) => t.taskId === taskId)
      ),
      candidateQuality: meanQuality(
        candidate.trials.filter((t) => t.taskId === taskId)
      ),
      delta:
        baselinePassRate !== undefined && candidatePassRate !== undefined
          ? candidatePassRate - baselinePassRate
          : undefined,
      ...(candidateFailureDetail !== undefined && { candidateFailureDetail }),
    };
  });

  const baselinePassRate = passRate(baseline.trials.map((t) => t.passed)) ?? 0;
  const candidatePassRate =
    passRate(candidate.trials.map((t) => t.passed)) ?? 0;

  return {
    baselineLabel: baseline.label,
    candidateLabel: candidate.label,
    tasks,
    baselinePassRate,
    candidatePassRate,
    passRateDelta: candidatePassRate - baselinePassRate,
    baselineQuality: meanQuality(baseline.trials),
    candidateQuality: meanQuality(candidate.trials),
    baselineSubcheckScore: meanSubcheckScore(baseline.trials),
    candidateSubcheckScore: meanSubcheckScore(candidate.trials),
    baselineTotalCost: baseline.totalCost,
    candidateTotalCost: candidate.totalCost,
    totalTokensDelta: candidate.totalTokens - baseline.totalTokens,
    totalCostDelta: candidate.totalCost - baseline.totalCost,
  };
}

export function formatArmComparison(comparison: ArmComparison): string {
  const header = [
    `## AX arm comparison: ${comparison.candidateLabel} vs ${comparison.baselineLabel}`,
    "",
    `| task | ${comparison.baselineLabel} | ${comparison.candidateLabel} | delta | quality (b→c) |`,
    "| --- | --- | --- | --- | --- |",
  ];
  const taskRows = comparison.tasks.map(
    (t) =>
      `| ${t.taskId} | ${formatRate(t.baselinePassRate)} | ${formatRate(t.candidatePassRate)} | ${formatDelta(t.delta)} | ${formatQuality(t.baselineQuality)}→${formatQuality(t.candidateQuality)} |`
  );
  const totals = [
    `| **all tasks** | ${formatRate(comparison.baselinePassRate)} | ${formatRate(comparison.candidatePassRate)} | ${formatDelta(comparison.passRateDelta)} | ${formatQuality(comparison.baselineQuality)}→${formatQuality(comparison.candidateQuality)} |`,
    "",
    `Run totals: tokens ${formatSigned(comparison.totalTokensDelta)}, cost ${formatSigned(comparison.totalCostDelta, "$")}`,
    "",
    "### Quality (graded, alongside the binary pass rate)",
    `| metric | ${comparison.baselineLabel} | ${comparison.candidateLabel} |`,
    "| --- | --- | --- |",
    `| judge quality (0–1) | ${formatQuality(comparison.baselineQuality)} | ${formatQuality(comparison.candidateQuality)} |`,
    `| subcheck partial credit | ${formatQuality(comparison.baselineSubcheckScore)} | ${formatQuality(comparison.candidateSubcheckScore)} |`,
    `| arm cost | ${formatArmCost(comparison.baselineTotalCost)} | ${formatArmCost(comparison.candidateTotalCost)} |`,
    `| quality per dollar | ${formatPerDollar(comparison.baselineQuality, comparison.baselineTotalCost)} | ${formatPerDollar(comparison.candidateQuality, comparison.candidateTotalCost)} |`,
  ];
  const regressions = comparison.tasks
    .filter(
      (t) =>
        t.delta !== undefined &&
        t.delta < 0 &&
        t.candidateFailureDetail !== undefined
    )
    .map((t) => `- ${t.taskId}: ${firstLine(t.candidateFailureDetail ?? "")}`);
  const regressionSection =
    regressions.length > 0
      ? ["", "### Regressed task diagnostics", ...regressions]
      : [];
  return [...header, ...taskRows, ...totals, ...regressionSection].join("\n");
}

function groupPassesByTask(
  trials: readonly ArmTrial[]
): Map<string, boolean[]> {
  const byTask = new Map<string, boolean[]>();
  for (const trial of trials) {
    const existing = byTask.get(trial.taskId) ?? [];
    existing.push(trial.passed);
    byTask.set(trial.taskId, existing);
  }
  return byTask;
}

function passRate(passes: readonly boolean[] | undefined): number | undefined {
  if (passes === undefined || passes.length === 0) {
    return undefined;
  }
  return passes.filter(Boolean).length / passes.length;
}

function formatRate(rate: number | undefined): string {
  return rate === undefined ? "n/a" : `${(rate * 100).toFixed(0)}%`;
}

function formatDelta(delta: number | undefined): string {
  if (delta === undefined) {
    return "n/a";
  }
  const points = (delta * 100).toFixed(0);
  return delta > 0 ? `+${points}pp` : `${points}pp`;
}

function formatSigned(value: number, unit = ""): string {
  const rendered =
    unit === "$" ? `$${Math.abs(value).toFixed(4)}` : `${Math.abs(value)}`;
  return value >= 0 ? `+${rendered}` : `-${rendered}`;
}

function firstLine(text: string): string {
  return text.split("\n", 1)[0] ?? "";
}

function meanQuality(trials: readonly ArmTrial[]): number | undefined {
  const judged = trials
    .map((t) => t.quality)
    .filter((q): q is number => q !== undefined);
  if (judged.length === 0) {
    return undefined;
  }
  return judged.reduce((sum, q) => sum + q, 0) / judged.length;
}

function meanSubcheckScore(trials: readonly ArmTrial[]): number | undefined {
  const scored = trials
    .map((t) => t.subcheckScore)
    .filter((s): s is number => s !== undefined);
  if (scored.length === 0) {
    return undefined;
  }
  return scored.reduce((sum, s) => sum + s, 0) / scored.length;
}

function formatQuality(quality: number | undefined): string {
  return quality === undefined ? "n/a" : quality.toFixed(2);
}

function formatArmCost(cost: number): string {
  return cost > 0 ? `$${cost.toFixed(4)}` : "n/a";
}

function formatPerDollar(quality: number | undefined, cost: number): string {
  if (quality === undefined || cost <= 0) {
    return "n/a";
  }
  return (quality / cost).toFixed(2);
}
