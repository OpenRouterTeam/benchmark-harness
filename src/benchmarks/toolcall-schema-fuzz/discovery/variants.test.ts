import { describe, expect, it } from "bun:test";

import { validateJsonSchema } from "../json-schema";
import { simplerVariants, widerVariants } from "./variants";

const PARAMETERS = {
  type: "object",
  properties: {
    a: { $ref: "#/$defs/maybe" },
    b: { type: "array", items: { type: ["string", "integer"] } },
  },
  required: ["a", "b"],
  additionalProperties: false,
  $defs: { maybe: { anyOf: [{ type: "integer" }, { type: "null" }] } },
};
const ARGS = { a: 7, b: ["xy", 3] };

describe("simplerVariants", () => {
  const variants = simplerVariants({ parameters: PARAMETERS, args: ARGS });

  it("only yields pairs whose arguments still validate", () => {
    expect(variants.length).toBeGreaterThan(0);
    for (const variant of variants) {
      expect(validateJsonSchema(variant.args, variant.parameters)).toEqual([]);
      expect(variant.parameters["type"]).toBe("object");
    }
  });

  it("inlines refs, picks the branch holding the value, drops keys and shrinks arrays", () => {
    const byEdit = new Map(variants.map((variant) => [variant.edit, variant]));
    expect(byEdit.get(".a: inline $ref")?.parameters).toEqual({
      ...PARAMETERS,
      properties: {
        ...PARAMETERS.properties,
        a: { anyOf: [{ type: "integer" }, { type: "null" }] },
      },
      $defs: undefined,
    });
    expect(byEdit.get("drop .b")?.args).toEqual({ a: 7 });
    expect(byEdit.get(".b: keep first item")?.args).toEqual({
      a: 7,
      b: ["xy"],
    });
    expect(byEdit.get(".a: zero number")?.args).toEqual({ a: 0, b: ["xy", 3] });
    expect(byEdit.has("drop .a")).toBe(true);
  });

  it("removes $defs no longer referenced", () => {
    const inlined = variants.find(
      (variant) => variant.edit === ".a: inline $ref"
    );
    expect(inlined?.parameters["$defs"]).toBeUndefined();
  });
});

describe("widerVariants", () => {
  it("wraps each property while keeping the arguments valid", () => {
    const variants = widerVariants({ parameters: PARAMETERS, args: ARGS });
    expect(variants.map((variant) => variant.edit)).toEqual([
      ".a: nullable",
      ".a: nullable-first",
      ".a: wrap in array",
      ".a: wrap in object",
      ".a: move to $defs",
      ".b: nullable",
      ".b: nullable-first",
      ".b: wrap in array",
      ".b: wrap in object",
      ".b: move to $defs",
    ]);
    for (const variant of variants) {
      expect(validateJsonSchema(variant.args, variant.parameters)).toEqual([]);
    }
    expect(variants[2]?.args).toEqual({ a: [7], b: ["xy", 3] });
  });
});
