import { describe, expect, it } from "bun:test";

import { rewriteProblems, schemaSkeleton, valueSkeleton } from "./skeleton";

const ORIGINAL = {
  name: "Qz",
  parameters: {
    type: "object",
    properties: {
      "aB-c": { $ref: "#/$defs/n" },
      zz: { anyOf: [{ type: "string", maxLength: 9 }, { type: "null" }] },
    },
    required: ["aB-c", "zz"],
    additionalProperties: false,
    $defs: { n: { type: "integer", enum: [3, 9] } },
  },
  args: [
    { "aB-c": 3, zz: "x\u0001" },
    { "aB-c": 9, zz: null },
  ],
} as const;

const REWRITE = {
  name: "pause_campaign",
  parameters: {
    type: "object",
    properties: {
      campaign_id: {
        $ref: "#/$defs/campaign",
        description: "Campaign to pause.",
      },
      reason: { anyOf: [{ type: "string", maxLength: 40 }, { type: "null" }] },
    },
    required: ["reason", "campaign_id"],
    additionalProperties: false,
    $defs: { campaign: { type: "integer", enum: [4821, 4822] } },
  },
  args: [
    { campaign_id: 4821, reason: "holiday promo ended" },
    { campaign_id: 4822, reason: null },
  ],
} as const;

describe("schemaSkeleton", () => {
  it("ignores names, descriptions, literals and bounds", () => {
    expect(schemaSkeleton(REWRITE.parameters)).toBe(
      schemaSkeleton(ORIGINAL.parameters)
    );
  });

  it("distinguishes branch order, types, required fields and enum length", () => {
    const base = schemaSkeleton(ORIGINAL.parameters);
    const swapped = {
      ...ORIGINAL.parameters,
      properties: {
        ...ORIGINAL.parameters.properties,
        zz: { anyOf: [{ type: "null" }, { type: "string", maxLength: 9 }] },
      },
    };
    expect(schemaSkeleton(swapped)).not.toBe(base);
    expect(
      schemaSkeleton({ ...ORIGINAL.parameters, required: ["aB-c"] })
    ).not.toBe(base);
    expect(
      schemaSkeleton({
        ...ORIGINAL.parameters,
        $defs: { n: { type: "integer", enum: [3] } },
      })
    ).not.toBe(base);
    expect(
      schemaSkeleton({
        ...ORIGINAL.parameters,
        $defs: { n: { type: "number", enum: [3, 9] } },
      })
    ).not.toBe(base);
  });
});

describe("valueSkeleton", () => {
  it("keeps JSON types, key order and array lengths but not keys or literals", () => {
    expect(valueSkeleton({ a: [1, "x"], b: null })).toBe(
      valueSkeleton({ c: [7, "y"], d: null })
    );
    expect(valueSkeleton({ a: 1 })).not.toBe(valueSkeleton({ a: 1.5 }));
    expect(valueSkeleton({ a: [1] })).not.toBe(valueSkeleton({ a: [1, 2] }));
    expect(valueSkeleton({ a: 1, b: "x" })).not.toBe(
      valueSkeleton({ b: "x", a: 1 })
    );
  });
});

describe("rewriteProblems", () => {
  it("accepts a faithful rewrite", () => {
    expect(
      rewriteProblems({
        original: ORIGINAL,
        rewrite: REWRITE,
        distractorName: "resume_campaign",
      })
    ).toEqual([]);
  });

  it("rejects changed branches, invalid arguments, bad names and collapsed draws", () => {
    const problems = rewriteProblems({
      original: ORIGINAL,
      rewrite: {
        ...REWRITE,
        name: "pause campaign",
        args: [
          { campaign_id: 1, reason: null },
          { campaign_id: 1, reason: null },
        ],
      },
      distractorName: "pause campaign",
    });
    expect(problems).toEqual([
      "tool name pause campaign is not a valid name",
      "distractor name must be a valid name different from the tool name",
      expect.stringContaining("args[0] {"),
      "args[0] must keep the original JSON type, key order and array length at every position",
      expect.stringContaining("args[1] {"),
      "args[0] and args[1] must differ",
    ]);
  });

  it("rejects integers JSON cannot carry exactly", () => {
    const unsafe = 2 ** 53 + 2;
    const problems = rewriteProblems({
      original: ORIGINAL,
      rewrite: {
        ...REWRITE,
        parameters: {
          ...REWRITE.parameters,
          $defs: { campaign: { type: "integer", enum: [unsafe, 4822] } },
        },
        args: [
          { campaign_id: unsafe, reason: "holiday promo ended" },
          REWRITE.args[1],
        ],
      },
      distractorName: "resume_campaign",
    });
    expect(problems).toEqual([
      "integers must stay within ±2^53 so JSON keeps every digit",
    ]);
  });

  it("rejects a schema whose structure changed", () => {
    const problems = rewriteProblems({
      original: ORIGINAL,
      rewrite: {
        ...REWRITE,
        parameters: { ...REWRITE.parameters, additionalProperties: true },
      },
      distractorName: "resume_campaign",
    });
    expect(problems).toEqual([
      "schema structure differs from the original: keep every keyword, type, branch and order",
    ]);
  });
});

describe("schemaSkeleton for applicator and object keywords", () => {
  it("erases names and bounds but keeps escaped refs, dependencies and subschemas", () => {
    const original = {
      type: "object",
      properties: { a: { $ref: "#/definitions/x~1y" }, b: { type: "string" } },
      dependentRequired: { a: ["b"] },
      patternProperties: { "^x_": { not: { type: "string" } } },
      minProperties: 1,
      definitions: { "x/y": { type: "integer", default: 3 } },
    };
    const renamed = {
      type: "object",
      properties: {
        order: { $ref: "#/definitions/ids~1order" },
        note: { type: "string" },
      },
      dependentRequired: { order: ["note"] },
      patternProperties: { "^tag_": { not: { type: "string" } } },
      minProperties: 2,
      definitions: { "ids/order": { type: "integer", default: 41 } },
    };
    expect(schemaSkeleton(renamed)).toBe(schemaSkeleton(original));
    const unescaped = {
      ...renamed,
      properties: {
        ...renamed.properties,
        order: { $ref: "#/definitions/order" },
      },
      definitions: { order: { type: "integer", default: 41 } },
    };
    expect(schemaSkeleton(unescaped)).not.toBe(schemaSkeleton(original));
    expect(
      schemaSkeleton({ ...renamed, dependentRequired: { note: ["order"] } })
    ).not.toBe(schemaSkeleton(original));
    expect(
      schemaSkeleton({
        ...renamed,
        patternProperties: { "^tag_": { not: { type: "integer" } } },
      })
    ).not.toBe(schemaSkeleton(original));
  });

  it("keeps dependentSchemas triggers and required keys anchored to parent properties", () => {
    const properties = { a: { type: "string" }, b: { type: "string" } };
    const original = {
      type: "object",
      properties,
      dependentSchemas: { a: { required: ["b"] } },
    };
    const renamed = {
      type: "object",
      properties: { order: { type: "string" }, note: { type: "string" } },
      dependentSchemas: { order: { required: ["note"] } },
    };
    const swapped = {
      ...renamed,
      dependentSchemas: { note: { required: ["order"] } },
    };
    expect(schemaSkeleton(renamed)).toBe(schemaSkeleton(original));
    expect(schemaSkeleton(swapped)).not.toBe(schemaSkeleton(original));
  });
});
