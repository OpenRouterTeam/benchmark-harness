import type {
  CriterionVerdict,
  DracoTask,
  TaskScore,
  TaskVerdicts,
} from "./schemas";

export interface RunScoreResult {
  readonly rawScore: number;
  readonly maxPossible: number;
  readonly normalizedScore: number;
  readonly passRate: number;
  readonly criteriaMet: number;
  readonly criteriaTotal: number;
}

export function scoreRun(
  verdicts: readonly CriterionVerdict[]
): RunScoreResult {
  let rawScore = 0;
  let maxPossible = 0;
  let criteriaPassed = 0;
  for (const v of verdicts) {
    const w = v.weight;
    const met = v.verdict === "MET";
    if (w > 0) {
      maxPossible += w;
      if (met) {
        rawScore += w;
        criteriaPassed += 1;
      }
    } else if (w < 0) {
      if (met) {
        rawScore += w;
      } else {
        criteriaPassed += 1;
      }
    }
  }
  const normalizedScore =
    maxPossible === 0
      ? 0
      : Math.max(0, Math.min(1, rawScore / maxPossible)) * 100;
  const passRate =
    verdicts.length > 0 ? (criteriaPassed / verdicts.length) * 100 : 0;
  return {
    rawScore,
    maxPossible,
    normalizedScore,
    passRate,
    criteriaMet: criteriaPassed,
    criteriaTotal: verdicts.length,
  };
}

export function aggregateTaskScores(
  task: DracoTask,
  taskVerdicts: TaskVerdicts,
  experimentName: string
): TaskScore {
  const runScores: (RunScoreResult & {
    runNum: number;
  })[] = [];
  for (const run of taskVerdicts.runs) {
    if (run.status !== "ok" || run.verdicts.length === 0) {
      continue;
    }
    runScores.push({ ...scoreRun(run.verdicts), runNum: run.runNum });
  }
  const judgeRunsCompleted = taskVerdicts.runs.filter(
    (r) => r.status === "ok"
  ).length;
  const judgeRunsFailed = taskVerdicts.runs.filter(
    (r) => r.status === "failed"
  ).length;
  const judgingCost = taskVerdicts.runs.reduce(
    (sum, r) => sum + (r.cost ?? 0),
    0
  );
  if (runScores.length === 0) {
    return {
      taskId: task.id,
      domain: task.domain,
      experimentName,
      runScores: [],
      meanNormalized: 0,
      stdNormalized: 0,
      meanPassRate: 0,
      stdPassRate: 0,
      judgeRunsCompleted,
      judgeRunsFailed,
      judgingCost,
      totalCost: judgingCost,
    };
  }
  const normScores = runScores.map((r) => r.normalizedScore);
  const passRates = runScores.map((r) => r.passRate);
  return {
    taskId: task.id,
    domain: task.domain,
    experimentName,
    runScores: runScores.map((r) => ({
      runNum: r.runNum,
      rawScore: r.rawScore,
      maxPossible: r.maxPossible,
      normalizedScore: r.normalizedScore,
      passRate: r.passRate,
      criteriaMet: r.criteriaMet,
      criteriaTotal: r.criteriaTotal,
    })),
    meanNormalized: mean(normScores),
    stdNormalized: std(normScores),
    meanPassRate: mean(passRates),
    stdPassRate: std(passRates),
    judgeRunsCompleted,
    judgeRunsFailed,
    judgingCost,
    totalCost: judgingCost,
  };
}

export interface DomainAggregate {
  readonly meanNormalized: number;
  readonly stdNormalized: number;
  readonly judgeNoise: number;
  readonly meanPassRate: number;
  readonly tasksScored: number;
  readonly tasksWithFailures: number;
  readonly generationCost: number;
  readonly judgingCost: number;
  readonly totalCost: number;
}

export interface SessionAggregate {
  readonly overall: DomainAggregate;
  readonly byDomain: Readonly<Record<string, DomainAggregate>>;
}

export function aggregateSessionScores(
  taskScores: readonly TaskScore[]
): SessionAggregate {
  if (taskScores.length === 0) {
    return { overall: emptyAggregate(), byDomain: {} };
  }
  const byDomain = new Map<string, TaskScore[]>();
  for (const ts of taskScores) {
    const list = byDomain.get(ts.domain) ?? [];
    list.push(ts);
    byDomain.set(ts.domain, list);
  }
  const byDomainOut: Record<string, DomainAggregate> = {};
  for (const [domain, scores] of [...byDomain.entries()].sort(([a], [b]) =>
    a.localeCompare(b)
  )) {
    byDomainOut[domain] = aggregateGroup(scores);
  }
  return {
    overall: aggregateGroup(taskScores),
    byDomain: byDomainOut,
  };
}

function aggregateGroup(scores: readonly TaskScore[]): DomainAggregate {
  const norm = scores.map((s) => s.meanNormalized);
  const pr = scores.map((s) => s.meanPassRate);
  const perTaskJudgeStds = scores.map((s) => s.stdNormalized);
  const generationCost = scores.reduce(
    (sum, s) => sum + (s.generationCost ?? 0),
    0
  );
  const judgingCost = scores.reduce((sum, s) => sum + s.judgingCost, 0);
  const totalCost = scores.reduce((sum, s) => sum + s.totalCost, 0);
  return {
    meanNormalized: mean(norm),
    stdNormalized: std(norm),
    judgeNoise: mean(perTaskJudgeStds),
    meanPassRate: mean(pr),
    tasksScored: scores.length,
    tasksWithFailures: scores.filter((s) => s.judgeRunsFailed > 0).length,
    generationCost,
    judgingCost,
    totalCost,
  };
}

function emptyAggregate(): DomainAggregate {
  return {
    meanNormalized: 0,
    stdNormalized: 0,
    judgeNoise: 0,
    meanPassRate: 0,
    tasksScored: 0,
    tasksWithFailures: 0,
    generationCost: 0,
    judgingCost: 0,
    totalCost: 0,
  };
}

function mean(values: readonly number[]): number {
  return values.length > 0
    ? values.reduce((a, b) => a + b, 0) / values.length
    : 0;
}

function std(values: readonly number[]): number {
  if (values.length < 2) {
    return 0;
  }
  const m = mean(values);
  const variance =
    values.reduce((acc, v) => acc + (v - m) ** 2, 0) / values.length;
  return Math.sqrt(variance);
}
