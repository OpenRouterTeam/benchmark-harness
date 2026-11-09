import { describe, expect, it } from "bun:test";

import { validateJsonSchema } from "./json-schema";

const NULLABLE_NUMBER = { anyOf: [{ type: "number" }, { type: "null" }] };

describe("validateJsonSchema", () => {
  it("rejects numeric strings against anyOf [number, null]", () => {
    expect(validateJsonSchema("40", NULLABLE_NUMBER)).toEqual([
      { path: "$", message: 'string "40" matched no anyOf branch' },
    ]);
    expect(validateJsonSchema("null", NULLABLE_NUMBER)).toHaveLength(1);
    expect(validateJsonSchema(40, NULLABLE_NUMBER)).toEqual([]);
    expect(validateJsonSchema(null, NULLABLE_NUMBER)).toEqual([]);
  });

  it("handles type arrays, integers, enums and bounds", () => {
    expect(validateJsonSchema(null, { type: ["integer", "null"] })).toEqual([]);
    expect(validateJsonSchema(1.5, { type: "integer" })).toHaveLength(1);
    expect(validateJsonSchema(4, { type: "integer", enum: [1, 2, 4] })).toEqual(
      []
    );
    expect(validateJsonSchema(3, { type: "integer", enum: [1, 2, 4] })).toEqual(
      [{ path: "$", message: "number is not one of [1,2,4]" }]
    );
    expect(
      validateJsonSchema(6, { type: "integer", minimum: 1, maximum: 5 })
    ).toEqual([{ path: "$", message: "6 violates maximum 5" }]);
  });

  it("reports missing required, unexpected and nested item violations", () => {
    const schema = {
      type: "object",
      additionalProperties: false,
      required: ["path", "items"],
      properties: {
        path: { type: "string" },
        items: { type: "array", items: { type: "number" } },
      },
    };
    expect(
      validateJsonSchema({ items: [1, "2"], extra: true }, schema)
    ).toEqual([
      { path: "$.path", message: "required property missing" },
      { path: "$.items[1]", message: 'expected number, got string "2"' },
      { path: "$.extra", message: "unexpected property" },
    ]);
  });

  it("resolves local refs and applies oneOf, allOf and const", () => {
    const schema = {
      type: "object",
      $defs: { address: { type: "object", required: ["city"] } },
      properties: {
        shipping: { $ref: "#/$defs/address" },
        shape: {
          oneOf: [
            { properties: { kind: { const: "circle" } }, required: ["kind"] },
            { properties: { kind: { const: "rect" } }, required: ["kind"] },
          ],
        },
        user: {
          allOf: [
            { type: "object", required: ["name"] },
            { type: "object", properties: { age: { type: "integer" } } },
          ],
        },
      },
    };
    expect(
      validateJsonSchema(
        {
          shipping: { city: "Springfield" },
          shape: { kind: "rect" },
          user: { name: "Bo", age: 41 },
        },
        schema
      )
    ).toEqual([]);
    expect(
      validateJsonSchema(
        { shipping: {}, shape: { kind: "oval" }, user: { age: "41" } },
        schema
      )
    ).toEqual([
      { path: "$.shipping.city", message: "required property missing" },
      { path: "$.shape", message: "object matched 0 oneOf branches" },
      { path: "$.user.name", message: "required property missing" },
      { path: "$.user.age", message: 'expected integer, got string "41"' },
    ]);
    expect(validateJsonSchema({}, { $ref: "#/$defs/missing" })).toEqual([
      { path: "$", message: "unresolvable $ref #/$defs/missing" },
    ]);
  });

  it("checks validation keywords beyond type", () => {
    expect(validateJsonSchema("ab", { type: "string", minLength: 3 })).toEqual([
      { path: "$", message: "2 violates minLength 3" },
    ]);
    expect(
      validateJsonSchema("😀😀", { type: "string", maxLength: 2 })
    ).toEqual([]);
    expect(
      validateJsonSchema("a.b", { type: "string", pattern: String.raw`^a\.b$` })
    ).toEqual([]);
    expect(
      validateJsonSchema("axb", { type: "string", pattern: String.raw`^a\.b$` })
    ).toEqual([
      { path: "$", message: String.raw`string "axb" does not match ^a\.b$` },
    ]);
    expect(
      validateJsonSchema(2, { type: "number", exclusiveMaximum: 2 })
    ).toHaveLength(1);
    expect(validateJsonSchema(9, { type: "integer", multipleOf: 3 })).toEqual(
      []
    );
    expect(
      validateJsonSchema(10, { type: "integer", multipleOf: 3 })
    ).toHaveLength(1);
    expect(validateJsonSchema([], { type: "array", minItems: 1 })).toHaveLength(
      1
    );
  });

  it("checks prefixItems, items false and additionalProperties schemas", () => {
    const tuple = {
      type: "array",
      prefixItems: [{ type: "string" }, { type: "integer" }],
      items: false,
    };
    expect(validateJsonSchema(["a", 1], tuple)).toEqual([]);
    expect(validateJsonSchema(["a", "1", true], tuple)).toEqual([
      { path: "$[1]", message: 'expected integer, got string "1"' },
      { path: "$[2]", message: "unexpected item" },
    ]);
    const map = { type: "object", additionalProperties: { type: "integer" } };
    expect(validateJsonSchema({ a: 1, b: "2" }, map)).toEqual([
      { path: "$.b", message: 'expected integer, got string "2"' },
    ]);
  });

  it("follows recursive references", () => {
    const tree = {
      $ref: "#/$defs/node",
      $defs: {
        node: {
          type: "object",
          properties: {
            children: { type: "array", items: { $ref: "#/$defs/node" } },
          },
          required: ["children"],
        },
      },
    };
    expect(validateJsonSchema({ children: [{ children: [] }] }, tree)).toEqual(
      []
    );
    expect(validateJsonSchema({ children: [{}] }, tree)).toEqual([
      { path: "$.children[0].children", message: "required property missing" },
    ]);
  });

  it("checks contains, minContains, maxContains and uniqueItems", () => {
    const schema = {
      contains: { type: "integer" },
      minContains: 1,
      maxContains: 2,
    };
    expect(validateJsonSchema(["a", 1], schema)).toEqual([]);
    expect(validateJsonSchema(["a"], schema)).toHaveLength(1);
    expect(validateJsonSchema([1, 2, 3], schema)).toHaveLength(1);
    expect(
      validateJsonSchema(
        [
          { a: 1, b: 2 },
          { b: 2, a: 1 },
        ],
        { uniqueItems: true }
      )
    ).toHaveLength(1);
    expect(validateJsonSchema([1, "1"], { uniqueItems: true })).toEqual([]);
    expect(validateJsonSchema([1, 2, "a"], schema)).toEqual([]);
    expect(validateJsonSchema([[1], [1]], { uniqueItems: true })).toHaveLength(
      1
    );
    expect(validateJsonSchema([[1], [2]], { uniqueItems: true })).toEqual([]);
    expect(validateJsonSchema([1, 1], { uniqueItems: false })).toEqual([]);
  });

  it("checks patternProperties, propertyNames and property counts", () => {
    const schema = {
      patternProperties: { "^x_": { type: "integer" } },
      additionalProperties: false,
      propertyNames: { maxLength: 4 },
      minProperties: 1,
      maxProperties: 2,
    };
    expect(validateJsonSchema({ x_a: 1 }, schema)).toEqual([]);
    expect(validateJsonSchema({ x_a: "one" }, schema)).toHaveLength(1);
    expect(validateJsonSchema({ y: 1 }, schema)).toHaveLength(1);
    expect(validateJsonSchema({ x_long: 1 }, schema)).toHaveLength(1);
    expect(validateJsonSchema({}, schema)).toHaveLength(1);
    expect(validateJsonSchema({ x_a: 1, x_b: 2, x_c: 3 }, schema)).toHaveLength(
      1
    );
  });

  it("checks dependentRequired and dependentSchemas", () => {
    expect(
      validateJsonSchema({ a: 1 }, { dependentRequired: { a: ["b"] } })
    ).toHaveLength(1);
    expect(
      validateJsonSchema({ a: 1, b: 2 }, { dependentRequired: { a: ["b"] } })
    ).toEqual([]);
    expect(
      validateJsonSchema({ b: 2 }, { dependentRequired: { a: ["b"] } })
    ).toEqual([]);
    const schema = { dependentSchemas: { a: { required: ["b"] } } };
    expect(validateJsonSchema({ a: 1 }, schema)).toHaveLength(1);
    expect(validateJsonSchema({ a: 1, b: 2 }, schema)).toEqual([]);
    expect(validateJsonSchema({ c: 1 }, schema)).toEqual([]);
    expect(
      validateJsonSchema({}, { dependentRequired: { toString: ["id"] } })
    ).toEqual([]);
    expect(
      validateJsonSchema(
        {},
        { dependentSchemas: { toString: { required: ["id"] } } }
      )
    ).toEqual([]);
    expect(validateJsonSchema({}, { required: ["toString"] })).toHaveLength(1);
  });

  it("applies not and if/then/else", () => {
    expect(validateJsonSchema(1, { not: { type: "string" } })).toEqual([]);
    expect(validateJsonSchema("x", { not: { type: "string" } })).toHaveLength(
      1
    );
    const schema = {
      if: { type: "integer" },
      // oxlint-disable-next-line unicorn/no-thenable -- `then` is the JSON Schema conditional keyword, not a thenable
      then: { minimum: 5 },
      else: { type: "string" },
    };
    expect(validateJsonSchema(6, schema)).toEqual([]);
    expect(validateJsonSchema(4, schema)).toHaveLength(1);
    expect(validateJsonSchema("x", schema)).toEqual([]);
    expect(validateJsonSchema(true, schema)).toHaveLength(1);
  });

  it("compares const and enum by JSON value and resolves definitions and escaped pointers", () => {
    expect(
      validateJsonSchema({ b: [1], a: 2 }, { const: { a: 2, b: [1] } })
    ).toEqual([]);
    expect(validateJsonSchema({ a: 2 }, { enum: [{ a: 3 }] })).toHaveLength(1);
    const schema = {
      $ref: "#/definitions/api~1Model~0v1%20x",
      definitions: { "api/Model~v1 x": { type: "integer" } },
    };
    expect(validateJsonSchema(1, schema)).toEqual([]);
    expect(validateJsonSchema("1", schema)).toHaveLength(1);
  });
});
