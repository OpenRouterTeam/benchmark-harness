import { describe, expect, it } from "bun:test";

import {
  regressionCases,
  TOOLCALL_SCHEMA_FUZZ_REGRESSIONS,
} from "./regressions";
import { FuzzConstruct } from "./tool-case";

const TOOL = {
  name: "fuzz",
  strict: true,
  parameters: {
    type: "object",
    properties: { x: { $ref: "#/$defs/n" } },
    required: ["x"],
    additionalProperties: false,
    $defs: { n: { type: "integer" } },
  },
  args: { x: 20 },
};
const CONTROL = {
  ...TOOL,
  parameters: {
    type: "object",
    properties: { x: { type: "integer" } },
    required: ["x"],
    additionalProperties: false,
  },
};

describe("regressionCases", () => {
  it("emits each finding and its control as linked single-call cases", () => {
    const cases = regressionCases({
      version: 2,
      entries: [
        {
          key: "novita-ref-int",
          failing: TOOL,
          control: CONTROL,
          finding: {
            category: "schema_violation",
            explanation:
              'schema_violation: $.x expected integer, got string "20"',
            model: "z-ai/glm-5.3-flash",
            provider: "novita",
            observed: { failures: 3, runs: 3 },
            reference: {
              provider: "fireworks",
              observed: { failures: 0, runs: 3 },
            },
            discoveredAt: "2026-10-05T00:00:00.000Z",
          },
        },
      ],
    });
    expect(
      cases.map(({ id, control, index }) => ({ id, control, index }))
    ).toEqual([
      {
        id: "toolcall_schema_fuzz-regression:novita-ref-int",
        control: "toolcall_schema_fuzz-regression:novita-ref-int:control",
        index: 0,
      },
      {
        id: "toolcall_schema_fuzz-regression:novita-ref-int:control",
        control: undefined,
        index: 1,
      },
    ]);
    expect(cases[0]?.constructs).toContain(FuzzConstruct.Ref);
    expect(cases[0]?.calls).toEqual([{ name: "fuzz", args: { x: 20 } }]);
    expect(cases[0]?.messages[0]).toEqual({
      role: "user",
      content: expect.stringContaining('"x": 20'),
    });
    expect(cases[0]?.messages[1]).toMatchObject({
      tool_calls: [{ function: { name: "fuzz", arguments: '{"x":20}' } }],
    });
  });

  it("parses the committed regression file", () => {
    expect(TOOLCALL_SCHEMA_FUZZ_REGRESSIONS.version).toBeGreaterThanOrEqual(1);
    expect(regressionCases(TOOLCALL_SCHEMA_FUZZ_REGRESSIONS)).toHaveLength(
      TOOLCALL_SCHEMA_FUZZ_REGRESSIONS.entries.reduce(
        (sum, entry) => sum + (entry.control === undefined ? 1 : 2),
        0
      )
    );
  });
});
