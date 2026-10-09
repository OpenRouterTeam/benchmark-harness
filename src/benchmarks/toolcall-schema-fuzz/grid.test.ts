import { describe, expect, it } from "bun:test";

import { constructsOf } from "./constructs";
import { GRID_SLOTS, gridShapeDraws, slotCaseId } from "./grid";
import { validateJsonSchema } from "./json-schema";
import { FuzzConstruct } from "./tool-case";

const PREFIX = "grid:";
const KEYS = GRID_SLOTS.map((slot) =>
  slotCaseId({ prefix: PREFIX, slot, draw: 0 })
);

function shapeKeys(prefix: string): ReadonlySet<string> {
  return new Set(
    KEYS.flatMap((key) =>
      key.startsWith(prefix) ? [key.slice(prefix.length)] : []
    )
  );
}

describe("GRID_SLOTS", () => {
  it("enumerates every single-call shape and every leaf under each multi-message scenario", () => {
    expect(new Set(KEYS).size).toBe(KEYS.length);
    const counts = Object.groupBy(
      GRID_SLOTS,
      (slot) => `${slot.scenario}:${slot.strict ? "strict" : "loose"}`
    );
    expect(
      Object.fromEntries(
        Object.entries(counts).map(([key, list]) => [key, list?.length])
      )
    ).toEqual({
      "single:loose": 1361,
      "single:strict": 559,
      "replay:loose": 171,
      "replay:strict": 104,
      "distractor:loose": 171,
      "distractor:strict": 104,
      "parallel:loose": 171,
      "parallel:strict": 104,
    });
  });

  it("marks slots strict only for strict-compatible shapes", () => {
    const loose = shapeKeys(`${PREFIX}loose:`);
    const strictKeys = [...shapeKeys(`${PREFIX}strict:`)];
    expect(strictKeys.filter((key) => !loose.has(key))).toEqual([]);
    for (const key of strictKeys) {
      expect(key).not.toMatch(
        /const<|\{\}@|oneOf|allOf|obj\(opt|,open\)|map<|prefixItems|minLength/
      );
    }
  });

  it("covers each union branch in both orders, every wrapper pair and each keyword leaf", () => {
    const loose = shapeKeys(`${PREFIX}loose:`);
    for (const key of [
      "type[integer,null]@null",
      "type[null,integer]@null",
      "anyOf[string,boolean]@boolean",
      "oneOf[number,integer]@number",
      "nullable-first/ref/boolean",
      "nullable@null/string",
      "array/nullable-first@null/integer",
      "array/obj(opt,open)/null",
      "obj(any-key)/ref/number",
      "allOf/{}@object",
      "string{pattern=literal}",
      "string{format=date-time}",
      "number{exclusiveMinimum,exclusiveMaximum}",
      "prefixItems[string,integer,boolean]",
      "map<boolean>",
      "tree",
      "replay:anyOf[integer,string]@integer",
      "parallel:string",
      "distractor:string{description}",
    ]) {
      expect({ key, present: loose.has(key) }).toEqual({ key, present: true });
    }
    expect(loose.has("oneOf[number,integer]@integer")).toBe(false);
    expect(loose.has("replay:array/string")).toBe(false);
  });
});

describe("gridShapeDraws", () => {
  const draws = gridShapeDraws();

  it("draws arguments that satisfy the schema of each shape", () => {
    const invalid = draws.filter(({ drawn }) =>
      drawn.args.some(
        (args) => validateJsonSchema(args, drawn.parameters).length > 0
      )
    );
    expect(invalid.map(({ shape }) => shape.key)).toEqual([]);
  });

  it("derives every construct tag from some shape", () => {
    const covered = new Set(
      draws.flatMap(({ drawn }) => constructsOf(drawn.parameters))
    );
    expect(
      Object.values(FuzzConstruct).filter(
        (construct) => !covered.has(construct)
      )
    ).toEqual([]);
    const single = draws.find(
      ({ shape }) => shape.key === "type[integer,null]@null"
    );
    expect(
      single === undefined ? [] : constructsOf(single.drawn.parameters)
    ).toEqual([
      FuzzConstruct.Object,
      FuzzConstruct.TypeArray,
      FuzzConstruct.Integer,
      FuzzConstruct.Null,
      FuzzConstruct.Nullable,
    ]);
  });
});
