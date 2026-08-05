import { describe, expect, it } from "bun:test";

import { extractCriteria } from "./criteria";
import type { Criterion } from "./schemas";
describe("extractCriteria", () => {
  it("flattens top-level criteria carrying their section id/title", () => {
    const rubric = {
      id: "task-1",
      sections: [
        {
          id: "factual-accuracy",
          title: "Factual Accuracy",
          criteria: [
            { id: "c1", weight: 10, requirement: "States X" },
            { id: "c2", weight: -5, requirement: "Recommends home management" },
          ],
        },
      ],
    };
    const result = extractCriteria(rubric);
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({
      id: "c1",
      section: "Factual Accuracy",
      sectionId: "factual-accuracy",
      weight: 10,
      requirement: "States X",
    } satisfies Criterion);
    expect(result[1]!.weight).toBe(-5);
  });
  it("walks nested sub-sections, inheriting the deepest section metadata", () => {
    const rubric = {
      sections: [
        {
          id: "breadth",
          title: "Breadth & Depth",
          criteria: [{ id: "top", weight: 3, requirement: "r" }],
          sections: [
            {
              id: "breadth-sub",
              title: "Coverage",
              criteria: [{ id: "nested", weight: 4, requirement: "r2" }],
            },
          ],
        },
      ],
    };
    const result = extractCriteria(rubric);
    expect(result.map((c) => c.id).sort()).toEqual(["nested", "top"]);
    const nested = result.find((c) => c.id === "nested")!;
    expect(nested.section).toBe("Coverage");
    expect(nested.sectionId).toBe("breadth-sub");
    const top = result.find((c) => c.id === "top")!;
    expect(top.section).toBe("Breadth & Depth");
  });
  it("coerces numeric-string weights and defaults missing ones to 0", () => {
    const rubric = {
      sections: [
        {
          id: "s",
          title: "S",
          criteria: [
            { id: "a", weight: "7", requirement: "r" },
            { id: "b", weight: "not-a-number", requirement: "r" },
            { id: "c", requirement: "r" },
          ],
        },
      ],
    };
    const result = extractCriteria(rubric);
    expect(result[0]!.weight).toBe(7);
    expect(result[1]!.weight).toBe(0);
    expect(result[2]!.weight).toBe(0);
  });
  it("returns empty for a rubric with no sections/criteria", () => {
    expect(extractCriteria({})).toEqual([]);
    expect(extractCriteria({ sections: [] })).toEqual([]);
    expect(extractCriteria({ sections: [{ id: "s", title: "S" }] })).toEqual(
      []
    );
  });
});
