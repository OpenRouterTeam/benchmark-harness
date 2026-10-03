import { isRecord } from "../../internal/guards";

export type JsonSchema = Readonly<Record<string, unknown>>;

export interface SchemaViolation {
  readonly path: string;
  readonly message: string;
}

interface SchemaLocation {
  readonly schema: JsonSchema;
  readonly path: string;
}

function describeValue(value: unknown): string {
  if (value === null) {
    return "null";
  }
  if (Array.isArray(value)) {
    return "array";
  }
  if (typeof value === "string") {
    return `string ${JSON.stringify(value)}`;
  }
  return typeof value;
}

function matchesType(value: unknown, type: unknown): boolean {
  switch (type) {
    case "string": {
      return typeof value === "string";
    }
    case "number": {
      return typeof value === "number" && Number.isFinite(value);
    }
    case "integer": {
      return typeof value === "number" && Number.isInteger(value);
    }
    case "boolean": {
      return typeof value === "boolean";
    }
    case "null": {
      return value === null;
    }
    case "array": {
      return Array.isArray(value);
    }
    case "object": {
      return isRecord(value) && !Array.isArray(value);
    }
    default: {
      return false;
    }
  }
}

function schemaAt(schema: JsonSchema, key: string): JsonSchema | undefined {
  const value = schema[key];
  return isRecord(value) ? value : undefined;
}

function boundViolations(
  value: number,
  { schema, path }: SchemaLocation
): SchemaViolation[] {
  const minimum = schema["minimum"];
  const maximum = schema["maximum"];
  return [
    ...(typeof minimum === "number" && value < minimum
      ? [{ path, message: `${value} < minimum ${minimum}` }]
      : []),
    ...(typeof maximum === "number" && value > maximum
      ? [{ path, message: `${value} > maximum ${maximum}` }]
      : []),
  ];
}

function objectViolations(
  value: Readonly<Record<string, unknown>>,
  { schema, path }: SchemaLocation
): SchemaViolation[] {
  const properties = schemaAt(schema, "properties") ?? {};
  const required = schema["required"];
  const missing = (Array.isArray(required) ? required : [])
    .filter((key): key is string => typeof key === "string" && !(key in value))
    .map((key) => ({
      path: `${path}.${key}`,
      message: "required property missing",
    }));
  const children = Object.entries(value).flatMap(([key, child]) => {
    const childSchema = properties[key];
    if (isRecord(childSchema)) {
      return validateAt(child, { schema: childSchema, path: `${path}.${key}` });
    }
    return schema["additionalProperties"] === false
      ? [{ path: `${path}.${key}`, message: "unexpected property" }]
      : [];
  });
  return [...missing, ...children];
}

function validateAt(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const { schema, path } = location;
  const anyOf = schema["anyOf"];
  if (Array.isArray(anyOf)) {
    const isMatched = anyOf
      .filter(isRecord)
      .some(
        (branch) => validateAt(value, { schema: branch, path }).length === 0
      );
    return isMatched
      ? []
      : [{ path, message: `${describeValue(value)} matched no anyOf branch` }];
  }
  const type = schema["type"];
  if (type !== undefined) {
    const types = Array.isArray(type) ? type : [type];
    if (!types.some((candidate) => matchesType(value, candidate))) {
      return [
        {
          path,
          message: `expected ${types.join(" | ")}, got ${describeValue(value)}`,
        },
      ];
    }
  }
  const enumValues = schema["enum"];
  if (Array.isArray(enumValues) && !enumValues.includes(value)) {
    return [
      {
        path,
        message: `${describeValue(value)} is not one of ${JSON.stringify(enumValues)}`,
      },
    ];
  }
  if (typeof value === "number") {
    return boundViolations(value, location);
  }
  if (Array.isArray(value)) {
    const items = schemaAt(schema, "items");
    return items === undefined
      ? []
      : value.flatMap((item: unknown, index) =>
          validateAt(item, { schema: items, path: `${path}[${index}]` })
        );
  }
  return isRecord(value) ? objectViolations(value, location) : [];
}

export function validateJsonSchema(
  value: unknown,
  schema: JsonSchema
): readonly SchemaViolation[] {
  return validateAt(value, { schema, path: "$" });
}
