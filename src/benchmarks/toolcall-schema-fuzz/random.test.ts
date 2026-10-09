import { describe, expect, it } from "bun:test";

import { sample } from "fast-check";

import { constructsOf } from "./constructs";
import { validateJsonSchema } from "./json-schema";
import { parametersOf, randomDraws } from "./random";
import { keywordPositions } from "./schema-positions";
import { REQUIRED_KEYWORD_POSITIONS } from "./spec-keywords";
import { FuzzConstruct } from "./tool-case";

const COVERAGE_DRAWS = 2500;
const COVERAGE_SEED = 3;

const COVERAGE_TIMEOUT_MS = 30_000;
const DRAWS = 200;
const RANDOM_CONSTRUCTS = [
  FuzzConstruct.String,
  FuzzConstruct.Integer,
  FuzzConstruct.Number,
  FuzzConstruct.Boolean,
  FuzzConstruct.Null,
  FuzzConstruct.Enum,
  FuzzConstruct.Const,
  FuzzConstruct.AnySchema,
  FuzzConstruct.TypeArray,
  FuzzConstruct.Nullable,
  FuzzConstruct.Object,
  FuzzConstruct.OpenObject,
  FuzzConstruct.OptionalProperty,
  FuzzConstruct.Array,
  FuzzConstruct.AnyOf,
  FuzzConstruct.OneOf,
  FuzzConstruct.AllOf,
  FuzzConstruct.Ref,
  FuzzConstruct.RecursiveRef,
  FuzzConstruct.StringBounds,
  FuzzConstruct.Pattern,
  FuzzConstruct.Format,
  FuzzConstruct.NumberBounds,
  FuzzConstruct.MultipleOf,
  FuzzConstruct.ArrayBounds,
  FuzzConstruct.PrefixItems,
  FuzzConstruct.AdditionalPropertiesSchema,
  FuzzConstruct.Description,
  FuzzConstruct.UnusualKey,
  FuzzConstruct.Not,
  FuzzConstruct.Conditional,
  FuzzConstruct.Contains,
  FuzzConstruct.UniqueItems,
  FuzzConstruct.PropertyBounds,
  FuzzConstruct.PatternProperties,
  FuzzConstruct.PropertyNames,
  FuzzConstruct.DependentRequired,
  FuzzConstruct.DependentSchemas,
  FuzzConstruct.Annotation,
  FuzzConstruct.ContentAnnotation,
  FuzzConstruct.Definitions,
  FuzzConstruct.RefSiblings,
  FuzzConstruct.EscapedPointer,
  FuzzConstruct.SingleAllOf,
  FuzzConstruct.RootCombinator,
  FuzzConstruct.CompositeTypeArray,
  FuzzConstruct.BooleanSchema,
  FuzzConstruct.SingleBranchUnion,
  FuzzConstruct.WideCombinator,
] as const;
const TOOL_NAME = /^[a-zA-Z0-9_-]{1,64}$/;
const STRICT_FORBIDDEN = new Set(["oneOf", "allOf", "const"]);

function objectProblems(schema: object, path: string): readonly string[] {
  if (
    !("properties" in schema) ||
    typeof schema.properties !== "object" ||
    schema.properties === null
  ) {
    return [];
  }
  const required =
    "required" in schema && Array.isArray(schema.required)
      ? schema.required
      : [];
  const closed =
    "additionalProperties" in schema && schema.additionalProperties === false;
  return [
    ...(closed ? [] : [`${path} is not closed`]),
    ...Object.keys(schema.properties)
      .filter((key) => !required.includes(key))
      .map((key) => `${path}.${key} is not required`),
  ];
}

function strictProblems(schema: unknown, path: string): readonly string[] {
  if (Array.isArray(schema)) {
    return schema.flatMap((item, index) =>
      strictProblems(item, `${path}[${index}]`)
    );
  }
  if (typeof schema !== "object" || schema === null) {
    return [];
  }
  const entries = Object.entries(schema);
  const own = [
    ...(entries.length === 0 ? [`${path} is an empty schema`] : []),
    ...entries
      .filter(([key]) => STRICT_FORBIDDEN.has(key))
      .map(([key]) => `${path} uses ${key}`),
    ...objectProblems(schema, path),
  ];
  const nested = entries
    .filter(([key]) => key !== "enum" && key !== "required")
    .flatMap(([key, value]) =>
      key === "properties" && typeof value === "object" && value !== null
        ? Object.entries(value).flatMap(([name, child]) =>
            strictProblems(child, `${path}.${name}`)
          )
        : strictProblems(value, `${path}/${key}`)
    );
  return [...own, ...nested];
}

describe("randomDraws", () => {
  const draws = sample(randomDraws, { seed: 20_261_004, numRuns: DRAWS });

  it("draws arguments that satisfy the schema they come with", () => {
    for (const draw of draws) {
      expect(validateJsonSchema(draw.args, parametersOf(draw.node))).toEqual(
        []
      );
      expect(JSON.parse(JSON.stringify(draw.args))).toEqual(draw.args);
    }
  });

  it("uses valid tool names and only strict-compatible strict schemas", () => {
    expect(
      draws.map((draw) => draw.name).filter((name) => !TOOL_NAME.test(name))
    ).toEqual([]);
    const strictViolations = draws
      .map((draw, index) => ({ draw, index }))
      .filter(({ draw }) => draw.strict)
      .flatMap(({ draw, index }) =>
        strictProblems(parametersOf(draw.node), `draw ${index}`)
      );
    expect(strictViolations).toEqual([]);
  });

  it("covers both strict and non-strict tools and each schema construct", () => {
    const strict = draws.filter((draw) => draw.strict).length;
    expect(strict).toBeGreaterThan(DRAWS / 4);
    expect(strict).toBeLessThan((DRAWS * 3) / 4);
    const covered = new Set(
      draws.flatMap((draw) => constructsOf(parametersOf(draw.node)))
    );
    expect(
      RANDOM_CONSTRUCTS.filter((construct) => !covered.has(construct))
    ).toEqual([]);
  });

  it(
    "generates every spec keyword at every position SPEC_KEYWORDS lists",
    () => {
      const covered = new Set(
        sample(randomDraws, {
          seed: COVERAGE_SEED,
          numRuns: COVERAGE_DRAWS,
        }).flatMap((draw) => [...keywordPositions(parametersOf(draw.node))])
      );
      expect(
        REQUIRED_KEYWORD_POSITIONS.filter((pair) => !covered.has(pair))
      ).toEqual([]);
    },
    COVERAGE_TIMEOUT_MS
  );
});
