import { ScoreValue } from "../../harness/core";
import type { BenchmarkResultRow } from "../../results/parquet-schema";
import type { RowDiagnostics, RunTotals } from "./result-row-metrics";
import {
  diagnosticsFromMetadata,
  qualityFromMetadata,
  runTotalsFromResultRows,
  verdictKindFromMetadata,
} from "./result-row-metrics";
import type { FrictionDiagnostics, ResourceUsage } from "./trace";
import {
  agentEventStreamFromMessages,
  frictionFromEvents,
  parseTraceEvents,
  resourceUsageFromEvents,
} from "./trace";

export const AX_WEIGHTS = {
  correctness: 0.5,
  quality: 0.25,
  efficiency: 0.15,
  reliability: 0.1,
} as const;

const AX_COMPONENT_KEYS = [
  "correctness",
  "quality",
  "efficiency",
  "reliability",
] as const satisfies readonly (keyof typeof AX_WEIGHTS)[];

export type AxTrialCategory =
  | "pass"
  | "agent_failure"
  | "platform_failure"
  | "fixture_failure"
  | "skipped";

export interface AxTrial {
  readonly taskId: string;
  readonly epoch: number;
  readonly category: AxTrialCategory;
  readonly quality?: number;
  readonly subcheckScore?: number;
  readonly resources: ResourceUsage;
  readonly friction: FrictionDiagnostics;
  readonly diagnostics?: RowDiagnostics;
  readonly failureDetail?: string;
}

export interface AxComponents {
  readonly correctness?: number;
  readonly quality?: number;
  readonly efficiency?: number;
  readonly reliability?: number;
}

export interface AxCellScore {
  readonly ax: number | undefined;
  readonly components: AxComponents;
  readonly appliedWeights: AxComponents;
  readonly passRate: number | undefined;
  readonly trials: readonly AxTrial[];
  readonly counts: Readonly<Record<AxTrialCategory, number>>;
  readonly totalCost: number;
  readonly totalTokens: number;
  readonly costPerPass: number | undefined;
  readonly tokensPerPass: number | undefined;
}

export interface AxRoutingSummary {
  readonly totals: ResourceUsage;
  readonly friction: FrictionDiagnostics;
  readonly activation: {
    readonly mcp: number;
    readonly skills: number;
    readonly docs: number;
    readonly webFetch: number;
  };
  readonly liveSourceTrials: number;
  readonly memoryOnlyTrials: number;
  readonly trialCount: number;
}

const FIXTURE_FAILURE_LINE = /^VERIFY FAIL: FIXTURE STALE:/;

const PLATFORM_FAILURE_PATTERN =
  /HTTP 5\d\d|not retrievable|ECONNRESET|ETIMEDOUT|socket hang up|fetch failed/;

const VERIFIER_DIAGNOSTIC_LINE = /^VERIFY FAIL:/;

const SOLVER_ERROR_LINE = /^Solver error:/;

const SOLVER_CONFIG_ERROR_LINE = /^Solver error: benchmark config:/;

export function categorizeTrial(row: BenchmarkResultRow): AxTrialCategory {
  if (row.score_value === ScoreValue.Correct) {
    return "pass";
  }
  if (row.score_value === ScoreValue.Skipped) {
    return "skipped";
  }
  const verdictKind = verdictKindFromMetadata(row.metadata);
  if (verdictKind === "fixture") {
    return "fixture_failure";
  }
  if (verdictKind === "platform") {
    return "platform_failure";
  }
  const lines = (row.explanation ?? "").split("\n");
  if (lines.some((line) => FIXTURE_FAILURE_LINE.test(line))) {
    return "fixture_failure";
  }
  if (lines[0] !== undefined && SOLVER_CONFIG_ERROR_LINE.test(lines[0])) {
    return "fixture_failure";
  }
  if (lines[0] !== undefined && SOLVER_ERROR_LINE.test(lines[0])) {
    return "platform_failure";
  }
  const verifierLines = lines
    .filter((line) => VERIFIER_DIAGNOSTIC_LINE.test(line))
    .map(stripQuotedSegments);
  if (verifierLines.some((line) => PLATFORM_FAILURE_PATTERN.test(line))) {
    return "platform_failure";
  }
  return "agent_failure";
}

function stripQuotedSegments(line: string): string {
  return line.replaceAll(/"[^"]*"/g, '""');
}

export function axTrialsFromResultRows(
  rows: readonly BenchmarkResultRow[]
): AxTrial[] {
  return rows.map((row) => {
    const { quality, subcheckScore } = qualityFromMetadata(row.metadata);
    const diagnostics = diagnosticsFromMetadata(row.metadata);
    const category = categorizeTrial(row);
    const events = parseTraceEvents(agentEventStreamFromMessages(row.messages));
    return {
      taskId: row.sample_id,
      epoch: row.epoch,
      category,
      ...(quality !== undefined && { quality }),
      ...(subcheckScore !== undefined && { subcheckScore }),
      resources: resourceUsageFromEvents(events),
      friction: frictionFromEvents(events),
      ...(Object.keys(diagnostics).length > 0 && { diagnostics }),
      ...(category !== "pass" &&
        row.explanation !== null && { failureDetail: row.explanation }),
    };
  });
}

export function trialQuality(trial: AxTrial): number | undefined {
  if (trial.category === "pass") {
    return trial.quality;
  }
  if (trial.category === "agent_failure" && trial.subcheckScore !== undefined) {
    return trial.subcheckScore * 0.5;
  }
  return undefined;
}

export function didJudgeGradeTrials(trials: readonly AxTrial[]): boolean {
  return trials.some(
    (trial) => trial.category === "pass" && trial.quality !== undefined
  );
}

export function efficiencyScore(
  spendPerPass: number,
  refSpendPerPass: number
): number {
  if (refSpendPerPass <= 0 || spendPerPass <= 0) {
    return 1;
  }
  return Math.min(
    1,
    refSpendPerPass / Math.min(spendPerPass, refSpendPerPass * 4)
  );
}

export interface ComputeAxOpts {
  readonly refTokensPerPass?: number;
  readonly totals?: RunTotals;
}

export function computeAxScore(
  rows: readonly BenchmarkResultRow[],
  opts: ComputeAxOpts = {}
): AxCellScore {
  const trials = axTrialsFromResultRows(rows);
  const counts = {
    pass: 0,
    agent_failure: 0,
    platform_failure: 0,
    fixture_failure: 0,
    skipped: 0,
  };
  for (const trial of trials) {
    counts[trial.category] += 1;
  }

  const scoredTrials = counts.pass + counts.agent_failure;
  const correctness = scoredTrials > 0 ? counts.pass / scoredTrials : undefined;

  const qualityValues = didJudgeGradeTrials(trials)
    ? trials
        .map(trialQuality)
        .filter((value): value is number => value !== undefined)
    : [];
  const quality = mean(qualityValues);

  const nonFixtureTrials = scoredTrials + counts.platform_failure;
  const reliability =
    nonFixtureTrials > 0
      ? 1 - counts.platform_failure / nonFixtureTrials
      : undefined;

  const { totalCost, totalTokens } =
    opts.totals ?? runTotalsFromResultRows(rows);
  const costPerPass =
    counts.pass > 0 && totalCost > 0 ? totalCost / counts.pass : undefined;
  const tokensPerPass =
    counts.pass > 0 && totalTokens > 0 ? totalTokens / counts.pass : undefined;
  const efficiency =
    opts.refTokensPerPass !== undefined && tokensPerPass !== undefined
      ? efficiencyScore(tokensPerPass, opts.refTokensPerPass)
      : undefined;

  const components: AxComponents = {
    ...(correctness !== undefined && { correctness }),
    ...(quality !== undefined && { quality }),
    ...(efficiency !== undefined && { efficiency }),
    ...(reliability !== undefined && { reliability }),
  };
  const { ax, appliedWeights } = weightedComposite(components);

  return {
    ax,
    components,
    appliedWeights,
    passRate: correctness,
    trials,
    counts,
    totalCost,
    totalTokens,
    costPerPass,
    tokensPerPass,
  };
}

export function withEfficiencyReference(
  score: AxCellScore,
  refTokensPerPass: number
): AxCellScore {
  const efficiency =
    score.tokensPerPass !== undefined
      ? efficiencyScore(score.tokensPerPass, refTokensPerPass)
      : undefined;
  const { efficiency: _staleEfficiency, ...otherComponents } = score.components;
  const components: AxComponents = {
    ...otherComponents,
    ...(efficiency !== undefined && { efficiency }),
  };
  const { ax, appliedWeights } = weightedComposite(components);
  return { ...score, ax, components, appliedWeights };
}

export function routingSummary(trials: readonly AxTrial[]): AxRoutingSummary {
  let totals: ResourceUsage = {
    mcpToolCalls: 0,
    skillInvocations: 0,
    docsReads: 0,
    webFetches: 0,
  };
  let friction: FrictionDiagnostics = {
    toolCalls: 0,
    erroredToolCalls: 0,
    appRunRetries: 0,
  };
  for (const trial of trials) {
    totals = {
      mcpToolCalls: totals.mcpToolCalls + trial.resources.mcpToolCalls,
      skillInvocations:
        totals.skillInvocations + trial.resources.skillInvocations,
      docsReads: totals.docsReads + trial.resources.docsReads,
      webFetches: totals.webFetches + trial.resources.webFetches,
    };
    friction = {
      toolCalls: friction.toolCalls + trial.friction.toolCalls,
      erroredToolCalls:
        friction.erroredToolCalls + trial.friction.erroredToolCalls,
      appRunRetries: friction.appRunRetries + trial.friction.appRunRetries,
    };
  }
  const liveSourceTrials = trials.filter((trial) =>
    usedLiveSource(trial.resources)
  ).length;
  const activation = {
    mcp: trials.filter((trial) => trial.resources.mcpToolCalls > 0).length,
    skills: trials.filter((trial) => trial.resources.skillInvocations > 0)
      .length,
    docs: trials.filter((trial) => trial.resources.docsReads > 0).length,
    webFetch: trials.filter((trial) => trial.resources.webFetches > 0).length,
  };
  return {
    totals,
    friction,
    activation,
    liveSourceTrials,
    memoryOnlyTrials: trials.length - liveSourceTrials,
    trialCount: trials.length,
  };
}

export function medianTokensPerPass(
  cells: readonly { readonly tokensPerPass: number | undefined }[]
): number | undefined {
  const values = cells
    .map((cell) => cell.tokensPerPass)
    .filter((value): value is number => value !== undefined)
    .toSorted((a, b) => a - b);
  if (values.length === 0) {
    return undefined;
  }
  const mid = Math.floor(values.length / 2);
  return values.length % 2 === 1
    ? values[mid]
    : ((values[mid - 1] ?? 0) + (values[mid] ?? 0)) / 2;
}

function usedLiveSource(resources: ResourceUsage): boolean {
  return (
    resources.mcpToolCalls > 0 ||
    resources.skillInvocations > 0 ||
    resources.docsReads > 0 ||
    resources.webFetches > 0
  );
}

function weightedComposite(components: AxComponents): {
  ax: number | undefined;
  appliedWeights: AxComponents;
} {
  const entries = AX_COMPONENT_KEYS.flatMap((key) => {
    const value = components[key];
    const weight: number = AX_WEIGHTS[key];
    return value === undefined ? [] : [{ key, value, weight }];
  });
  const totalWeight = entries.reduce((sum, entry) => sum + entry.weight, 0);
  if (totalWeight === 0) {
    return { ax: undefined, appliedWeights: {} };
  }
  const ax =
    100 *
    entries.reduce(
      (sum, entry) => sum + (entry.weight / totalWeight) * entry.value,
      0
    );
  const appliedWeights = Object.fromEntries(
    entries.map((entry) => [entry.key, entry.weight / totalWeight])
  );
  return { ax, appliedWeights };
}

function mean(values: readonly number[]): number | undefined {
  if (values.length === 0) {
    return undefined;
  }
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}
