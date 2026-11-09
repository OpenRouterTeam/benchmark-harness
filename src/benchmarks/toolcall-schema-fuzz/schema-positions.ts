import type { ValueOf } from "../../internal/guards";
import { isRecord } from "../../internal/guards";
import type { JsonSchema } from "./json-schema";

export const SchemaPosition = {
  Root: "root",
  Property: "property",
  Item: "item",
  Branch: "branch",
  Definition: "definition",
  RefSibling: "ref_sibling",
} as const;
export type SchemaPosition = ValueOf<typeof SchemaPosition>;

const CHILD_POSITIONS: Readonly<Record<string, SchemaPosition>> = {
  additionalProperties: SchemaPosition.Property,
  propertyNames: SchemaPosition.Property,
  items: SchemaPosition.Item,
  contains: SchemaPosition.Item,
  not: SchemaPosition.Branch,
  if: SchemaPosition.Branch,
  // oxlint-disable-next-line unicorn/no-thenable -- `then` is the JSON Schema conditional keyword, not a thenable
  then: SchemaPosition.Branch,
  else: SchemaPosition.Branch,
};
const LIST_POSITIONS: Readonly<Record<string, SchemaPosition>> = {
  prefixItems: SchemaPosition.Item,
  anyOf: SchemaPosition.Branch,
  oneOf: SchemaPosition.Branch,
  allOf: SchemaPosition.Branch,
};
const MAP_POSITIONS: Readonly<Record<string, SchemaPosition>> = {
  properties: SchemaPosition.Property,
  patternProperties: SchemaPosition.Property,
  dependentSchemas: SchemaPosition.Property,
  $defs: SchemaPosition.Definition,
  definitions: SchemaPosition.Definition,
};

function childrenOf(
  keyword: string,
  value: unknown
): readonly [JsonSchema, SchemaPosition][] {
  const single = CHILD_POSITIONS[keyword];
  if (single !== undefined) {
    return isRecord(value) ? [[value, single]] : [];
  }
  const list = LIST_POSITIONS[keyword];
  if (list !== undefined) {
    return Array.isArray(value)
      ? value.filter(isRecord).map((child) => [child, list])
      : [];
  }
  const map = MAP_POSITIONS[keyword];
  return map !== undefined && isRecord(value)
    ? Object.values(value)
        .filter(isRecord)
        .map((child) => [child, map])
    : [];
}

function walk(schema: JsonSchema, position: SchemaPosition): readonly string[] {
  const hasRef = "$ref" in schema;
  return Object.entries(schema).flatMap(([keyword, value]) => [
    `${keyword}@${position}`,
    ...(hasRef && keyword !== "$ref"
      ? [`${keyword}@${SchemaPosition.RefSibling}`]
      : []),
    ...childrenOf(keyword, value).flatMap(([child, at]) => walk(child, at)),
  ]);
}

export function keywordPositions(parameters: JsonSchema): ReadonlySet<string> {
  return new Set(walk(parameters, SchemaPosition.Root));
}
