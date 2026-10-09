import { describe, expect, it } from "bun:test";

import { ScoreValue } from "../../harness/core";
import { Stability, stabilityReport } from "./stability";

const METADATA = JSON.stringify({
  scenario: "single",
  constructs: ["object", "integer"],
});
const CONTROLLED = JSON.stringify({
  scenario: "single",
  constructs: ["object", "$ref"],
  control: "a",
});

function row(
  sampleId: string,
  {
    epoch,
    value,
    explanation = null,
    metadata = METADATA,
  }: {
    readonly epoch: number;
    readonly value: string;
    readonly explanation?: string | null;
    readonly metadata?: string;
  }
): Parameters<typeof stabilityReport>[0][number] {
  return {
    sample_id: sampleId,
    epoch,
    score_value: value,
    explanation,
    metadata,
  };
}

describe("stabilityReport", () => {
  it("labels each case by k/n and keeps transport errors out of the count", () => {
    const report = stabilityReport([
      row("a", { epoch: 1, value: ScoreValue.Correct }),
      row("a", { epoch: 2, value: ScoreValue.Correct }),
      row("b", {
        epoch: 1,
        value: ScoreValue.Incorrect,
        explanation: 'value_mismatch: $.x expected 1, got "1"',
        metadata: CONTROLLED,
      }),
      row("b", { epoch: 2, value: ScoreValue.Correct, metadata: CONTROLLED }),
      row("b", {
        epoch: 3,
        value: ScoreValue.Skipped,
        explanation: "Model error (skipped): 429",
        metadata: CONTROLLED,
      }),
      row("c", {
        epoch: 1,
        value: ScoreValue.Incorrect,
        explanation: "Model error: 500",
      }),
      row("d", {
        epoch: 1,
        value: ScoreValue.Incorrect,
        explanation: "schema_violation: $.x",
      }),
    ]);
    expect(
      report.cases.map(({ id, runs, passes, transport, stability }) => ({
        id,
        runs,
        passes,
        transport,
        stability,
      }))
    ).toEqual([
      {
        id: "a",
        runs: 2,
        passes: 2,
        transport: 0,
        stability: Stability.PassedEveryRun,
      },
      {
        id: "b",
        runs: 2,
        passes: 1,
        transport: 1,
        stability: Stability.FailedSomeRuns,
      },
      {
        id: "c",
        runs: 0,
        passes: 0,
        transport: 1,
        stability: Stability.TransportOnly,
      },
      {
        id: "d",
        runs: 1,
        passes: 0,
        transport: 0,
        stability: Stability.FailedEveryRun,
      },
    ]);
    const flaky = report.cases[1];
    expect(flaky?.failures).toEqual({ value_mismatch: 1 });
    expect(flaky?.examples).toEqual({
      value_mismatch: 'value_mismatch: $.x expected 1, got "1"',
    });
    expect(flaky?.control).toBe("a");
    expect(flaky?.controlStability).toBe(Stability.PassedEveryRun);
    expect(report.totals).toEqual({
      key: "all",
      cases: 4,
      runs: 5,
      passes: 3,
      flaky: 1,
    });
    expect(report.constructs.find((group) => group.key === "$ref")).toEqual({
      key: "$ref",
      cases: 1,
      runs: 2,
      passes: 1,
      flaky: 1,
    });
    expect(report.scenarios).toEqual([
      { key: "single", cases: 4, runs: 5, passes: 3, flaky: 1 },
    ]);
  });

  it("tolerates rows without metadata", () => {
    const [entry] = stabilityReport([
      row("x", { epoch: 1, value: ScoreValue.Correct, metadata: "not json" }),
    ]).cases;
    expect(entry?.constructs).toEqual([]);
    expect(entry?.scenario).toBeUndefined();
  });
});
