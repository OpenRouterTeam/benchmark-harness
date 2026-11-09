import { isRecord } from "../../../internal/guards";
import type { JsonSchema } from "../json-schema";
import { validateJsonSchema } from "../json-schema";

const ERASED_NUMBERS = new Set([
  "minLength",
  "maxLength",
  "minimum",
  "maximum",
  "exclusiveMinimum",
  "exclusiveMaximum",
  "multipleOf",
  "minItems",
  "maxItems",
  "minContains",
  "maxContains",
  "minProperties",
  "maxProperties",
]);
const DROPPED = new Set(["description", "title", "$comment"]);
const SUBSCHEMA_LISTS = new Set(["anyOf", "oneOf", "allOf", "prefixItems"]);
const SUBSCHEMA_MAPS = new Set([
  "properties",
  "$defs",
  "definitions",
  "patternProperties",
  "dependentSchemas",
]);
const SUBSCHEMAS = new Set([
  "items",
  "additionalProperties",
  "not",
  "if",
  "then",
  "else",
  "contains",
  "propertyNames",
]);
const INSTANCES = new Set(["default", "examples"]);
const ESCAPED_POINTER = /~[01]|%[\dA-Fa-f]{2}/;
const TOOL_NAME = /^[A-Za-z][\w-]{0,63}$/;

function literal(value: unknown): unknown {
  if (typeof value === "string") {
    return "<string>";
  }
  if (typeof value === "number") {
    return Number.isInteger(value) ? "<integer>" : "<number>";
  }
  return value;
}

function pointerOf(container: string, name: string): string {
  const token = name.replaceAll("~", "~0").replaceAll("/", "~1");
  return `#/${container}/${encodeURIComponent(token).replaceAll("%7E", "~")}`;
}

function defIndex(root: JsonSchema): ReadonlyMap<string, number> {
  return new Map(
    ["$defs", "definitions"].flatMap((container) => {
      const defs = root[container];
      return (isRecord(defs) ? Object.keys(defs) : []).flatMap(
        (name, index) => [
          [`#/${container}/${name}`, index] as const,
          [pointerOf(container, name), index] as const,
        ]
      );
    })
  );
}

function refSkeleton(ctx: Context, ref: unknown): unknown {
  const index = ctx.defs.get(String(ref));
  if (index === undefined) {
    return ref;
  }
  return ESCAPED_POINTER.test(String(ref)) ? [index, "escaped"] : index;
}

function dependencySkeleton(schema: JsonSchema, value: unknown): unknown {
  const keys = isRecord(schema["properties"])
    ? Object.keys(schema["properties"])
    : [];
  return isRecord(value)
    ? Object.entries(value).map(([trigger, needed]) => [
        keys.indexOf(trigger),
        Array.isArray(needed)
          ? needed.map((key) => keys.indexOf(String(key)))
          : needed,
      ])
    : value;
}

function dependentSchemasSkeleton(ctx: Context, schema: JsonSchema): unknown {
  const value = schema["dependentSchemas"];
  const keys = isRecord(schema["properties"])
    ? Object.keys(schema["properties"])
    : [];
  return isRecord(value)
    ? Object.entries(value).map(([trigger, child]) => [
        keys.indexOf(trigger),
        skeleton(ctx, child),
        isRecord(child) && Array.isArray(child["required"])
          ? child["required"].map((key) => keys.indexOf(String(key))).toSorted()
          : null,
      ])
    : value;
}

interface Context {
  readonly defs: ReadonlyMap<string, number>;
}

function keywordSkeleton(
  ctx: Context,
  { schema, keyword }: { readonly schema: JsonSchema; readonly keyword: string }
): unknown {
  const value = schema[keyword];
  if (keyword === "dependentSchemas") {
    return dependentSchemasSkeleton(ctx, schema);
  }
  if (SUBSCHEMA_MAPS.has(keyword)) {
    return isRecord(value)
      ? Object.values(value).map((child) => skeleton(ctx, child))
      : value;
  }
  if (keyword === "dependentRequired") {
    return dependencySkeleton(schema, value);
  }
  if (INSTANCES.has(keyword)) {
    return valueShape(value);
  }
  if (keyword === "required") {
    const keys = isRecord(schema["properties"])
      ? Object.keys(schema["properties"])
      : [];
    return Array.isArray(value)
      ? value.map((key) => keys.indexOf(String(key))).toSorted()
      : value;
  }
  if (keyword === "$ref") {
    return refSkeleton(ctx, value);
  }
  if (keyword === "enum") {
    return Array.isArray(value) ? value.map(literal) : value;
  }
  if (keyword === "const") {
    return literal(value);
  }
  if (keyword === "pattern") {
    return "<pattern>";
  }
  if (ERASED_NUMBERS.has(keyword)) {
    return "<bound>";
  }
  if (SUBSCHEMA_LISTS.has(keyword)) {
    return Array.isArray(value)
      ? value.map((child) => skeleton(ctx, child))
      : value;
  }
  return SUBSCHEMAS.has(keyword) ? skeleton(ctx, value) : value;
}

function skeleton(ctx: Context, schema: unknown): unknown {
  if (!isRecord(schema)) {
    return schema;
  }
  return Object.fromEntries(
    Object.keys(schema)
      .filter((keyword) => !DROPPED.has(keyword))
      .toSorted()
      .map((keyword) => [keyword, keywordSkeleton(ctx, { schema, keyword })])
  );
}

export function schemaSkeleton(schema: JsonSchema): string {
  return JSON.stringify(skeleton({ defs: defIndex(schema) }, schema));
}

function valueShape(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(valueShape);
  }
  if (isRecord(value)) {
    return Object.values(value).map(valueShape);
  }
  return value === null || typeof value === "boolean" ? value : literal(value);
}

export function valueSkeleton(value: unknown): string {
  return JSON.stringify(valueShape(value));
}

export interface ToolDraw {
  readonly name: string;
  readonly parameters: JsonSchema;
  readonly args: readonly [
    Readonly<Record<string, unknown>>,
    Readonly<Record<string, unknown>>,
  ];
}

function hasUnsafeInteger(value: unknown): boolean {
  if (typeof value === "number") {
    return Number.isInteger(value) && !Number.isSafeInteger(value);
  }
  if (Array.isArray(value)) {
    return value.some(hasUnsafeInteger);
  }
  return (
    typeof value === "object" &&
    value !== null &&
    Object.values(value).some(hasUnsafeInteger)
  );
}

export function rewriteProblems({
  original,
  rewrite,
  distractorName,
}: {
  readonly original: ToolDraw;
  readonly rewrite: ToolDraw;
  readonly distractorName: string;
}): readonly string[] {
  const isDistinct = (draw: ToolDraw): boolean =>
    JSON.stringify(draw.args[0]) !== JSON.stringify(draw.args[1]);
  return [
    ...(TOOL_NAME.test(rewrite.name)
      ? []
      : [`tool name ${rewrite.name} is not a valid name`]),
    ...(TOOL_NAME.test(distractorName) && distractorName !== rewrite.name
      ? []
      : ["distractor name must be a valid name different from the tool name"]),
    ...(schemaSkeleton(rewrite.parameters) ===
    schemaSkeleton(original.parameters)
      ? []
      : [
          "schema structure differs from the original: keep every keyword, type, branch and order",
        ]),
    ...rewrite.args.flatMap((args, index) => [
      ...validateJsonSchema(args, rewrite.parameters).map(
        (violation) => `args[${index}] ${JSON.stringify(violation)}`
      ),
      ...(valueSkeleton(args) === valueSkeleton(original.args[index])
        ? []
        : [
            `args[${index}] must keep the original JSON type, key order and array length at every position`,
          ]),
    ]),
    ...(isDistinct(original) && !isDistinct(rewrite)
      ? ["args[0] and args[1] must differ"]
      : []),
    ...(rewrite.args.some(hasUnsafeInteger)
      ? ["integers must stay within ±2^53 so JSON keeps every digit"]
      : []),
  ];
}
