import type { Arbitrary } from "fast-check";

import type { ValueOf } from "../../../internal/guards";
import type { JsonSchema } from "../json-schema";
import { validateJsonSchema } from "../json-schema";
import type { Defs } from "../shape";

export interface SchemaNode {
  readonly schema: JsonSchema;
  readonly defs: Defs;
  readonly definitions: Defs;
}

export interface Context {
  readonly strict: boolean;
  readonly depth: number;
  readonly describe: (
    value: unknown,
    context: Context
  ) => Arbitrary<SchemaNode>;
}

export function leaf(schema: JsonSchema): SchemaNode {
  return { schema, defs: {}, definitions: {} };
}

export function withChildren(
  schema: JsonSchema,
  children: readonly SchemaNode[]
): SchemaNode {
  return {
    schema,
    defs: Object.assign({}, ...children.map((child) => child.defs)),
    definitions: Object.assign(
      {},
      ...children.map((child) => child.definitions)
    ),
  };
}

export function parametersOf(node: SchemaNode): JsonSchema {
  return {
    ...node.schema,
    ...(Object.keys(node.defs).length > 0 ? { $defs: node.defs } : {}),
    ...(Object.keys(node.definitions).length > 0
      ? { definitions: node.definitions }
      : {}),
  };
}

export function accepts(value: unknown, node: SchemaNode): boolean {
  return validateJsonSchema(value, parametersOf(node)).length === 0;
}

export function deeper(context: Context, by = 1): Context {
  return { ...context, depth: context.depth + by };
}

export const JsonType = {
  String: "string",
  Integer: "integer",
  Number: "number",
  Boolean: "boolean",
  Null: "null",
  Array: "array",
  Object: "object",
} as const;
export type JsonType = ValueOf<typeof JsonType>;

export function jsonTypeOf(value: unknown): JsonType {
  if (value === null) {
    return JsonType.Null;
  }
  if (Array.isArray(value)) {
    return JsonType.Array;
  }
  switch (typeof value) {
    case "string": {
      return JsonType.String;
    }
    case "number": {
      return Number.isInteger(value) ? JsonType.Integer : JsonType.Number;
    }
    case "boolean": {
      return JsonType.Boolean;
    }
    default: {
      return JsonType.Object;
    }
  }
}

export function rejectingTypes(value: unknown): readonly JsonType[] {
  const own = jsonTypeOf(value);
  return Object.values(JsonType).filter(
    (type) =>
      type !== own && !(own === JsonType.Integer && type === JsonType.Number)
  );
}
