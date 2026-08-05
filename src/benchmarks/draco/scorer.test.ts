import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { parseSchema, z } from "../../internal/zod";
import type {
  Criterion,
  CriterionVerdict,
  DracoTask,
  TaskVerdicts,
} from "./schemas";
import {
  aggregateSessionScores,
  aggregateTaskScores,
  scoreRun,
} from "./scorer";

function verdict(opts: {
  criterionId: string;
  weight: number;
  verdict: "MET" | "UNMET";
  section?: string;
}): CriterionVerdict {
  const { criterionId, weight, verdict, section = "Factual Accuracy" } = opts;
  return { criterionId, section, weight, verdict, justification: "" };
}
describe("scoreRun", () => {
  it("scores positive weights: MET adds weight, UNMET adds 0", () => {
    const result = scoreRun([
      verdict({ criterionId: "a", weight: 10, verdict: "MET" }),
      verdict({ criterionId: "b", weight: 8, verdict: "UNMET" }),
      verdict({ criterionId: "c", weight: 2, verdict: "MET" }),
    ]);
    expect(result.rawScore).toBe(12);
    expect(result.maxPossible).toBe(20);
    expect(result.normalizedScore).toBeCloseTo(60, 5);
    expect(result.passRate).toBeCloseTo((2 / 3) * 100, 5);
    expect(result.criteriaMet).toBe(2);
    expect(result.criteriaTotal).toBe(3);
  });
  it("treats negative weights as penalties: MET on negative subtracts |w|, UNMET passes", () => {
    const result = scoreRun([
      verdict({ criterionId: "pos", weight: 10, verdict: "MET" }),
      verdict({ criterionId: "neg", weight: -5, verdict: "MET" }),
      verdict({ criterionId: "neg2", weight: -3, verdict: "UNMET" }),
    ]);
    expect(result.rawScore).toBe(5);
    expect(result.maxPossible).toBe(10);
    expect(result.normalizedScore).toBeCloseTo(50, 5);
    expect(result.passRate).toBeCloseTo((2 / 3) * 100, 5);
  });
  it("clamps many-penalty responses to 0% (no negative normalized)", () => {
    const result = scoreRun([
      verdict({ criterionId: "pos", weight: 4, verdict: "UNMET" }),
      verdict({ criterionId: "neg", weight: -500, verdict: "MET" }),
    ]);
    expect(result.rawScore).toBe(-500);
    expect(result.maxPossible).toBe(4);
    expect(result.normalizedScore).toBe(0);
  });
  it("zero-weight criteria contribute nothing to score or pass-rate numerator", () => {
    const result = scoreRun([
      verdict({ criterionId: "pos", weight: 10, verdict: "MET" }),
      verdict({ criterionId: "zero", weight: 0, verdict: "MET" }),
    ]);
    expect(result.rawScore).toBe(10);
    expect(result.maxPossible).toBe(10);
    expect(result.normalizedScore).toBe(100);
    expect(result.passRate).toBeCloseTo(50, 5);
  });
  it("returns 0 for empty verdicts", () => {
    const result = scoreRun([]);
    expect(result.rawScore).toBe(0);
    expect(result.maxPossible).toBe(0);
    expect(result.normalizedScore).toBe(0);
    expect(result.passRate).toBe(0);
    expect(result.criteriaTotal).toBe(0);
  });
});

const PythonVerdictsSchema = z.object({
  task_id: z.string(),
  experiment_name: z.string(),
  judge_model: z.string(),
  target_runs: z.number(),
  runs: z.array(
    z.object({
      run_num: z.number(),
      status: z.enum(["ok", "failed", "skipped"]),
      verdicts: z.array(
        z.object({
          criterion_id: z.string(),
          section: z.string(),
          weight: z.number(),
          verdict: z.enum(["MET", "UNMET"]),
          justification: z.string(),
        })
      ),
      cost: z.number().nullable(),
    })
  ),
});

type PythonVerdicts = z.infer<typeof PythonVerdictsSchema>;

function pythonVerdictsToTs(raw: PythonVerdicts): TaskVerdicts {
  return {
    taskId: raw.task_id,
    experimentName: raw.experiment_name,
    judgeModel: raw.judge_model,
    targetRuns: raw.target_runs,
    runs: raw.runs.map((r) => ({
      runNum: r.run_num,
      status: r.status,
      verdicts: r.verdicts.map((v) => ({
        criterionId: v.criterion_id,
        section: v.section,
        weight: v.weight,
        verdict: v.verdict,
        justification: v.justification,
      })),
      cost: r.cost ?? undefined,
      judgeModel: raw.judge_model,
      timestamp: "",
    })),
  };
}

function fixtureTask(
  taskId: string,
  domain: string,
  criteria: Criterion[]
): DracoTask {
  return { id: taskId, problem: "", domain, criteria, rawAnswer: {} };
}
import pythonScore from "./__fixtures__/262b3145.score.json";
import pythonVerdicts from "./__fixtures__/262b3145.verdicts.json";
describe("aggregateTaskScores (golden parity vs Python)", () => {
  it("reproduces the Python scorer output for task 262b3145", () => {
    const parsed = parseSchema(PythonVerdictsSchema, pythonVerdicts);
    assertRight(parsed);
    const taskVerdicts = pythonVerdictsToTs(parsed.right);
    const task = fixtureTask(
      pythonVerdicts.task_id,
      pythonScore.domain,
      taskVerdicts.runs[0]!.verdicts.map((v) => ({
        id: v.criterionId,
        section: v.section,
        sectionId: "",
        weight: v.weight,
        requirement: "",
      }))
    );
    const tsScore = aggregateTaskScores(
      task,
      taskVerdicts,
      pythonScore.experiment_name
    );
    expect(tsScore.meanNormalized).toBeCloseTo(pythonScore.mean_normalized, 6);
    expect(tsScore.stdNormalized).toBeCloseTo(pythonScore.std_normalized, 6);
    expect(tsScore.meanPassRate).toBeCloseTo(pythonScore.mean_pass_rate, 6);
    expect(tsScore.stdPassRate).toBeCloseTo(pythonScore.std_pass_rate, 6);
    expect(tsScore.judgeRunsCompleted).toBe(pythonScore.judge_runs_completed);
    expect(tsScore.judgeRunsFailed).toBe(pythonScore.judge_runs_failed);
    expect(tsScore.judgingCost).toBeCloseTo(pythonScore.judging_cost, 6);
    const pyRun = pythonScore.run_scores[0]!;
    const tsRun = tsScore.runScores[0]!;
    expect(tsRun.runNum).toBe(pyRun.run_num);
    expect(tsRun.rawScore).toBeCloseTo(pyRun.raw_score, 6);
    expect(tsRun.maxPossible).toBeCloseTo(pyRun.max_possible, 6);
    expect(tsRun.normalizedScore).toBeCloseTo(pyRun.normalized_score, 6);
    expect(tsRun.passRate).toBeCloseTo(pyRun.pass_rate, 6);
    expect(tsRun.criteriaMet).toBe(pyRun.criteria_met);
    expect(tsRun.criteriaTotal).toBe(pyRun.criteria_total);
  });
});
describe("aggregateSessionScores", () => {
  it("groups by domain and aggregates overall", () => {
    const mk = (opts: {
      taskId: string;
      domain: string;
      norm: number;
      cost: number;
    }) => ({
      taskId: opts.taskId,
      domain: opts.domain,
      experimentName: "e",
      runScores: [],
      meanNormalized: opts.norm,
      stdNormalized: 0,
      meanPassRate: opts.norm,
      stdPassRate: 0,
      judgeRunsCompleted: 1,
      judgeRunsFailed: 0,
      generationCost: opts.cost,
      judgingCost: 0,
      totalCost: opts.cost,
    });
    const agg = aggregateSessionScores([
      mk({ taskId: "t1", domain: "Tech", norm: 80, cost: 1 }),
      mk({ taskId: "t2", domain: "Tech", norm: 60, cost: 2 }),
      mk({ taskId: "t3", domain: "Med", norm: 40, cost: 3 }),
    ]);
    expect(agg.byDomain["Tech"]!.meanNormalized).toBeCloseTo(70, 5);
    expect(agg.byDomain["Tech"]!.tasksScored).toBe(2);
    expect(agg.byDomain["Med"]!.meanNormalized).toBeCloseTo(40, 5);
    expect(agg.overall.meanNormalized).toBeCloseTo(60, 5);
    expect(agg.overall.totalCost).toBe(6);
    expect(agg.overall.tasksScored).toBe(3);
  });
  it("returns empty aggregate for no scores", () => {
    const agg = aggregateSessionScores([]);
    expect(agg.overall.tasksScored).toBe(0);
    expect(agg.overall.meanNormalized).toBe(0);
    expect(Object.keys(agg.byDomain)).toHaveLength(0);
  });
});
