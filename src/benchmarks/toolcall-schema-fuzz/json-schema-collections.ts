import { isRecord } from "../../internal/guards";
import type { JsonSchema } from "./json-schema";

export interface SchemaViolation {
  readonly path: string;
  readonly message: string;
}

export type Validate = (
  value: unknown,
  at: { readonly schema: JsonSchema; readonly path: string }
) => readonly SchemaViolation[];

interface Context {
  readonly schema: JsonSchema;
  readonly path: string;
  readonly validate: Validate;
}

function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (isRecord(value)) {
    const keys = Object.keys(value).toSorted();
    return `{${keys.map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

export function jsonEqual(left: unknown, right: unknown): boolean {
  return canonicalJson(left) === canonicalJson(right);
}

function numberAt(schema: JsonSchema, key: string): number | undefined {
  const value = schema[key];
  return typeof value === "number" ? value : undefined;
}

function subschema(schema: JsonSchema, key: string): JsonSchema | undefined {
  const value = schema[key];
  return isRecord(value) ? value : undefined;
}

function itemSchemaAt(
  schema: JsonSchema,
  index: number
): JsonSchema | false | undefined {
  const prefix = schema["prefixItems"];
  if (Array.isArray(prefix) && index < prefix.length) {
    const item: unknown = prefix[index];
    return isRecord(item) ? item : undefined;
  }
  const items = schema["items"];
  if (items === false) {
    return false;
  }
  return isRecord(items) ? items : undefined;
}

function itemViolations(
  value: readonly unknown[],
  context: Context
): readonly SchemaViolation[] {
  return value.flatMap((item: unknown, index) => {
    const path = `${context.path}[${index}]`;
    const itemSchema = itemSchemaAt(context.schema, index);
    if (itemSchema === false) {
      return [{ path, message: "unexpected item" }];
    }
    return itemSchema === undefined
      ? []
      : context.validate(item, { schema: itemSchema, path });
  });
}

function containsViolations(
  value: readonly unknown[],
  { schema, path, validate }: Context
): readonly SchemaViolation[] {
  const contains = subschema(schema, "contains");
  if (contains === undefined) {
    return [];
  }
  const matches = value.filter(
    (item, index) =>
      validate(item, { schema: contains, path: `${path}[${index}]` }).length ===
      0
  ).length;
  const min = numberAt(schema, "minContains") ?? 1;
  const max = numberAt(schema, "maxContains") ?? Number.POSITIVE_INFINITY;
  return matches < min || matches > max
    ? [
        {
          path,
          message: `${matches} items match contains, expected ${min}..${max}`,
        },
      ]
    : [];
}

function uniqueViolations(
  value: readonly unknown[],
  { schema, path }: Context
): readonly SchemaViolation[] {
  if (schema["uniqueItems"] !== true) {
    return [];
  }
  const distinct = new Set(value.map(canonicalJson)).size;
  return distinct === value.length
    ? []
    : [{ path, message: "items are not unique" }];
}

export function arrayViolations(
  value: readonly unknown[],
  context: Context
): readonly SchemaViolation[] {
  return [
    ...itemViolations(value, context),
    ...containsViolations(value, context),
    ...uniqueViolations(value, context),
  ];
}

function matchesPattern(pattern: string, key: string): boolean {
  return new RegExp(pattern, "u").test(key);
}

function patternSchemas(
  schema: JsonSchema,
  key: string
): readonly JsonSchema[] {
  const patterns = subschema(schema, "patternProperties") ?? {};
  return Object.entries(patterns).flatMap(([pattern, child]) =>
    isRecord(child) && matchesPattern(pattern, key) ? [child] : []
  );
}

function propertyViolations(
  value: Readonly<Record<string, unknown>>,
  { schema, path, validate }: Context
): readonly SchemaViolation[] {
  const properties = subschema(schema, "properties") ?? {};
  return Object.entries(value).flatMap(([key, child]) => {
    const at = `${path}.${key}`;
    const named = properties[key];
    const matched = [
      ...(isRecord(named) ? [named] : []),
      ...patternSchemas(schema, key),
    ];
    if (matched.length > 0) {
      return matched.flatMap((childSchema) =>
        validate(child, { schema: childSchema, path: at })
      );
    }
    const additional = schema["additionalProperties"];
    if (isRecord(additional)) {
      return validate(child, { schema: additional, path: at });
    }
    return additional === false
      ? [{ path: at, message: "unexpected property" }]
      : [];
  });
}

function requiredViolations(
  value: Readonly<Record<string, unknown>>,
  { schema, path }: Context
): readonly SchemaViolation[] {
  const required = schema["required"];
  const dependent = subschema(schema, "dependentRequired") ?? {};
  const implied = Object.entries(dependent).flatMap(([trigger, keys]) =>
    Object.hasOwn(value, trigger) && Array.isArray(keys) ? keys : []
  );
  return [
    ...new Set([...(Array.isArray(required) ? required : []), ...implied]),
  ]
    .filter(
      (key): key is string =>
        typeof key === "string" && !Object.hasOwn(value, key)
    )
    .map((key) => ({
      path: `${path}.${key}`,
      message: "required property missing",
    }));
}

function dependentSchemaViolations(
  value: Readonly<Record<string, unknown>>,
  { schema, path, validate }: Context
): readonly SchemaViolation[] {
  const dependent = subschema(schema, "dependentSchemas") ?? {};
  return Object.entries(dependent).flatMap(([trigger, child]) =>
    Object.hasOwn(value, trigger) && isRecord(child)
      ? validate(value, { schema: child, path })
      : []
  );
}

function keyViolations(
  value: Readonly<Record<string, unknown>>,
  { schema, path, validate }: Context
): readonly SchemaViolation[] {
  const names = subschema(schema, "propertyNames");
  const keys = Object.keys(value);
  const min = numberAt(schema, "minProperties") ?? 0;
  const max = numberAt(schema, "maxProperties") ?? Number.POSITIVE_INFINITY;
  return [
    ...(keys.length < min || keys.length > max
      ? [
          {
            path,
            message: `${keys.length} properties, expected ${min}..${max}`,
          },
        ]
      : []),
    ...(names === undefined
      ? []
      : keys.flatMap((key) =>
          validate(key, { schema: names, path: `${path}.${key}` })
        )),
  ];
}

export function objectViolations(
  value: Readonly<Record<string, unknown>>,
  context: Context
): readonly SchemaViolation[] {
  return [
    ...requiredViolations(value, context),
    ...propertyViolations(value, context),
    ...dependentSchemaViolations(value, context),
    ...keyViolations(value, context),
  ];
}
