import { describe, expect, it } from "bun:test";

import type { ArmRun } from "./compare";
import {
  armRunFromResultRows,
  compareArms,
  formatArmComparison,
} from "./compare";

function arm(
  label: string,
  trials: ArmRun["trials"],
  totals?: Partial<ArmRun>
): ArmRun {
  return { label, trials, totalTokens: 0, totalCost: 0, ...totals };
}

describe("compareArms", () => {
  it("computes per-task pass rates and deltas across epochs", () => {
    const baseline = arm("docs", [
      { taskId: "basic-completion", epoch: 0, passed: true },
      { taskId: "basic-completion", epoch: 1, passed: false },
      { taskId: "tool-calling-loop", epoch: 0, passed: false },
      { taskId: "tool-calling-loop", epoch: 1, passed: false },
    ]);
    const candidate = arm("skills", [
      { taskId: "basic-completion", epoch: 0, passed: true },
      { taskId: "basic-completion", epoch: 1, passed: true },
      { taskId: "tool-calling-loop", epoch: 0, passed: true },
      { taskId: "tool-calling-loop", epoch: 1, passed: false },
    ]);

    const comparison = compareArms(baseline, candidate);
    expect(comparison.baselinePassRate).toBeCloseTo(0.25, 10);
    expect(comparison.candidatePassRate).toBeCloseTo(0.75, 10);
    expect(comparison.passRateDelta).toBeCloseTo(0.5, 10);

    const basic = comparison.tasks.find((t) => t.taskId === "basic-completion");
    expect(basic?.baselinePassRate).toBeCloseTo(0.5, 10);
    expect(basic?.candidatePassRate).toBeCloseTo(1, 10);
    expect(basic?.delta).toBeCloseTo(0.5, 10);
  });

  it("marks tasks missing from one arm with undefined rates and delta", () => {
    const baseline = arm("docs", [
      { taskId: "only-in-baseline", epoch: 0, passed: true },
    ]);
    const candidate = arm("mcp", [
      { taskId: "only-in-candidate", epoch: 0, passed: false },
    ]);

    const comparison = compareArms(baseline, candidate);
    const onlyBaseline = comparison.tasks.find(
      (t) => t.taskId === "only-in-baseline"
    );
    const onlyCandidate = comparison.tasks.find(
      (t) => t.taskId === "only-in-candidate"
    );
    expect(onlyBaseline?.candidatePassRate).toBeUndefined();
    expect(onlyBaseline?.delta).toBeUndefined();
    expect(onlyCandidate?.baselinePassRate).toBeUndefined();
  });

  it("computes token and cost deltas from run totals", () => {
    const baseline = arm("docs", [], { totalTokens: 1000, totalCost: 0.5 });
    const candidate = arm("skills", [], { totalTokens: 800, totalCost: 0.3 });

    const comparison = compareArms(baseline, candidate);
    expect(comparison.totalTokensDelta).toBe(-200);
    expect(comparison.totalCostDelta).toBeCloseTo(-0.2, 10);
  });

  it("surfaces the candidate failure diagnostic on regressed tasks", () => {
    const baseline = arm("docs", [{ taskId: "t", epoch: 0, passed: true }]);
    const candidate = arm("skills", [
      {
        taskId: "t",
        epoch: 0,
        passed: false,
        failureDetail: "VERIFY FAIL: no generation\nmore",
      },
    ]);

    const comparison = compareArms(baseline, candidate);
    expect(comparison.tasks[0]?.candidateFailureDetail).toContain(
      "VERIFY FAIL"
    );

    const report = formatArmComparison(comparison);
    expect(report).toContain("### Regressed task diagnostics");
    expect(report).toContain("- t: VERIFY FAIL: no generation");
    expect(report).not.toContain("more");
  });
});

describe("formatArmComparison", () => {
  it("renders a markdown table with per-task and total rows", () => {
    const comparison = compareArms(
      arm("docs", [{ taskId: "basic-completion", epoch: 0, passed: false }]),
      arm("skills", [{ taskId: "basic-completion", epoch: 0, passed: true }])
    );
    const report = formatArmComparison(comparison);
    expect(report).toContain("## AX arm comparison: skills vs docs");
    expect(report).toContain("| basic-completion | 0% | 100% | +100pp |");
    expect(report).toContain("| **all tasks** | 0% | 100% | +100pp |");
  });

  it("reports quality, partial credit, and quality per dollar for two passing arms", () => {
    const comparison = compareArms(
      arm(
        "baseline",
        [
          {
            taskId: "t",
            epoch: 0,
            passed: true,
            quality: 0.5,
            subcheckScore: 1,
          },
        ],
        {
          totalCost: 0.5,
        }
      ),
      arm(
        "mcp",
        [
          {
            taskId: "t",
            epoch: 0,
            passed: true,
            quality: 0.9,
            subcheckScore: 1,
          },
        ],
        {
          totalCost: 1,
        }
      )
    );
    const report = formatArmComparison(comparison);

    expect(comparison.passRateDelta).toBe(0);
    expect(report).toContain("| judge quality (0–1) | 0.50 | 0.90 |");
    expect(report).toContain("| subcheck partial credit | 1.00 | 1.00 |");
    expect(report).toContain("| quality per dollar | 1.00 | 0.90 |");
  });

  it("renders quality as n/a when the run had no judge", () => {
    const comparison = compareArms(
      arm("baseline", [{ taskId: "t", epoch: 0, passed: true }]),
      arm("mcp", [{ taskId: "t", epoch: 0, passed: true }])
    );

    expect(comparison.candidateQuality).toBeUndefined();
    expect(formatArmComparison(comparison)).toContain(
      "| judge quality (0–1) | n/a | n/a |"
    );
  });
});

describe("armRunFromResultRows", () => {
  it("maps parquet rows into trials with failure diagnostics", () => {
    const row = {
      format_version: 1,
      task: "agent_dx",
      model: "openai/gpt-5.2",
      epochs: 1,
      temperature: 0,
      benchmark_config: null,
      created_at: "2026-07-27T00:00:00Z",
      accuracy: 0.5,
      total_questions: 2,
      correct_answers: 1,
      input_tokens: 100,
      output_tokens: 50,
      total_tokens: 150,
      reasoning_tokens: 0,
      total_cost: 0.01,
      generation_time_ms: 1000,
      extra_scores: null,
      sample_id: "basic-completion",
      epoch: 0,
      input: null,
      target: null,
      score_value: "C",
      answer: null,
      explanation: "VERIFY PASS",
      messages: null,
      metadata: null,
    };
    const failedRow = {
      ...row,
      sample_id: "tool-calling-loop",
      score_value: "I",
      explanation: "VERIFY FAIL: no tool_calls finish",
    };

    const run = armRunFromResultRows("docs", [row, failedRow]);
    expect(run.trials).toEqual([
      { taskId: "basic-completion", epoch: 0, passed: true },
      {
        taskId: "tool-calling-loop",
        epoch: 0,
        passed: false,
        failureDetail: "VERIFY FAIL: no tool_calls finish",
      },
    ]);
    expect(run.totalTokens).toBe(150);
    expect(run.totalCost).toBeCloseTo(0.01, 10);
  });

  it("sums run-level totals across parts of a multi-part result file", () => {
    const partA = {
      format_version: 1,
      task: "agent_dx",
      model: "openai/gpt-5.2",
      epochs: 1,
      temperature: 0,
      benchmark_config: null,
      created_at: "2026-07-27T00:00:00Z",
      accuracy: 1,
      total_questions: 1,
      correct_answers: 1,
      input_tokens: 100,
      output_tokens: 50,
      total_tokens: 150,
      reasoning_tokens: 0,
      total_cost: 0.01,
      generation_time_ms: 1000,
      extra_scores: null,
      sample_id: "basic-completion",
      epoch: 0,
      input: null,
      target: null,
      score_value: "C",
      answer: null,
      explanation: "VERIFY PASS",
      messages: null,
      metadata: null,
    };
    const partB = {
      ...partA,
      created_at: "2026-07-27T01:00:00Z",
      sample_id: "tool-calling-loop",
      total_tokens: 250,
      total_cost: 0.02,
    };
    const partBSibling = { ...partB, sample_id: "streaming-usage" };

    const run = armRunFromResultRows("docs", [partA, partB, partBSibling]);
    expect(run.totalTokens).toBe(400);
    expect(run.totalCost).toBeCloseTo(0.03, 10);
  });

  it("reads judge quality and subcheck partial credit out of row metadata", () => {
    const row = {
      format_version: 1,
      task: "agent_dx",
      model: "openai/gpt-5.2",
      epochs: 1,
      temperature: 0,
      benchmark_config: null,
      created_at: "2026-07-27T00:00:00Z",
      accuracy: 1,
      total_questions: 1,
      correct_answers: 1,
      input_tokens: 100,
      output_tokens: 50,
      total_tokens: 150,
      reasoning_tokens: 0,
      total_cost: 0.01,
      generation_time_ms: 1000,
      extra_scores: null,
      sample_id: "basic-completion",
      epoch: 0,
      input: null,
      target: null,
      score_value: "C",
      answer: null,
      explanation: "VERIFY PASS",
      messages: null,
      metadata: JSON.stringify({
        quality: 0.75,
        subchecksPassed: 3,
        subchecksTotal: 4,
      }),
    };

    const run = armRunFromResultRows("docs", [row]);
    expect(run.trials[0]?.quality).toBeCloseTo(0.75, 10);
    expect(run.trials[0]?.subcheckScore).toBeCloseTo(0.75, 10);
  });

  it("ignores unparseable metadata rather than failing the comparison", () => {
    const row = {
      format_version: 1,
      task: "agent_dx",
      model: "openai/gpt-5.2",
      epochs: 1,
      temperature: 0,
      benchmark_config: null,
      created_at: "2026-07-27T00:00:00Z",
      accuracy: 1,
      total_questions: 1,
      correct_answers: 1,
      input_tokens: 100,
      output_tokens: 50,
      total_tokens: 150,
      reasoning_tokens: 0,
      total_cost: 0.01,
      generation_time_ms: 1000,
      extra_scores: null,
      sample_id: "basic-completion",
      epoch: 0,
      input: null,
      target: null,
      score_value: "C",
      answer: null,
      explanation: "VERIFY PASS",
      messages: null,
      metadata: "not json",
    };

    expect(
      armRunFromResultRows("docs", [row]).trials[0]?.quality
    ).toBeUndefined();
  });
});
