import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { dracoRecordToSample } from "./dataset";
import type { Criterion } from "./schemas";
import { CriterionSchema } from "./schemas";

const RUBRIC = {
  id: "task-1",
  sections: [
    {
      id: "factual-accuracy",
      title: "Factual Accuracy",
      criteria: [
        { id: "c1", weight: 10, requirement: "States X" },
        { id: "c2", weight: -5, requirement: "Recommends home management" },
      ],
      sections: [
        {
          id: "breadth",
          title: "Breadth",
          criteria: [{ id: "c3", weight: 4, requirement: "Covers Y" }],
        },
      ],
    },
  ],
};

function baseRecord(answer: unknown): Record<string, unknown> {
  return {
    id: "0c2c668a-c3bf-41af-93c9-b5614ff63508",
    problem: "Explain DiD.",
    domain: "Academic",
    answer,
  };
}
describe("dracoRecordToSample", () => {
  it("flattens a stringified rubric into metadata.criteria and preserves identity", () => {
    const sample = dracoRecordToSample(baseRecord(JSON.stringify(RUBRIC)), 7);
    expect(sample.id).toBe("0c2c668a-c3bf-41af-93c9-b5614ff63508");
    expect(sample.input).toBe("Explain DiD.");
    expect(sample.target).toEqual({ text: "" });
    expect(sample.metadata).toMatchObject({ domain: "Academic", index: 7 });
    const criteriaResult = parseSchema(
      CriterionSchema.array(),
      sample.metadata!["criteria"]
    );
    assertRight(criteriaResult);
    const criteria: Criterion[] = criteriaResult.right;
    expect(criteria.map((c) => c.id).sort()).toEqual(["c1", "c2", "c3"]);
    expect(criteria.find((c) => c.id === "c3")!.weight).toBe(4);
  });
  it("accepts a pre-parsed (object) answer", () => {
    const sample = dracoRecordToSample(baseRecord(RUBRIC), 0);
    const criteriaResult = parseSchema(
      CriterionSchema.array(),
      sample.metadata!["criteria"]
    );
    assertRight(criteriaResult);
    expect(criteriaResult.right).toHaveLength(3);
  });
  it("degrades gracefully on a missing/unparsable answer (empty criteria)", () => {
    const sample = dracoRecordToSample(baseRecord(undefined), 0);
    expect(sample.metadata!["criteria"]).toEqual([]);
    expect(sample.metadata!["domain"]).toBe("Academic");
  });
  it("falls back to an index-derived id when the row lacks one", () => {
    const sample = dracoRecordToSample(
      { problem: "q", domain: "d", answer: "{}" },
      3
    );
    expect(sample.id).toBe("draco-3");
  });
});
