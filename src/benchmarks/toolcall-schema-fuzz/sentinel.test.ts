import { describe, expect, it } from "bun:test";

import { caseLookup, toolCallSchemaFuzzCases } from "./cases";
import { ToolCallSchemaFuzzGenerator } from "./options";
import { TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES as GRID } from "./realistic/wordings";

describe("sentinel generator", () => {
  const sentinel = toolCallSchemaFuzzCases({
    generator: ToolCallSchemaFuzzGenerator.Sentinel,
  });
  const ids = caseLookup(sentinel);

  it("keeps every scenario × strict × construct seen in the realistic grid, with controls", () => {
    const wanted = new Set(
      GRID.flatMap((entry) =>
        entry.constructs.map(
          (construct) => `${entry.scenario}:${entry.strict}:${construct}`
        )
      )
    );
    const covered = new Set(
      sentinel.flatMap((entry) =>
        entry.constructs.map(
          (construct) => `${entry.scenario}:${entry.strict}:${construct}`
        )
      )
    );
    expect([...wanted].filter((key) => !covered.has(key))).toEqual([]);
    expect(sentinel.length).toBeLessThan(GRID.length / 10);
    const controls = new Set(
      sentinel.flatMap((entry) =>
        entry.control === undefined ? [] : [entry.control]
      )
    );
    const orphans = sentinel.filter(
      (entry) =>
        entry.control !== undefined &&
        !controls.has(entry.id) &&
        !ids.has(entry.control)
    );
    expect(orphans.map((entry) => entry.id)).toEqual([]);
    expect(sentinel.map((entry) => entry.index)).toEqual(
      sentinel.map((_, index) => index)
    );
  });
});
