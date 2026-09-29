import { isRecord } from "../../internal/guards";

export type JsonSchema = Readonly<Record<string, unknown>>;

export interface SchemaViolation {
  readonly path: string;
  readonly message: string;
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

export function validateJsonSchema(
  value: unknown,
  schema: JsonSchema,
  path = "$"
): readonly SchemaViolation[] {
  const anyOf = schema["anyOf"];
  if (Array.isArray(anyOf)) {
    const branches = anyOf.filter(isRecord);
    const matched = branches.some(
      (branch) => validateJsonSchema(value, branch, path).length === 0
    );
    return matched
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
  const violations: SchemaViolation[] = [];
  if (typeof value === "number") {
    const minimum = schema["minimum"];
    const maximum = schema["maximum"];
    if (typeof minimum === "number" && value < minimum) {
      violations.push({ path, message: `${value} < minimum ${minimum}` });
    }
    if (typeof maximum === "number" && value > maximum) {
      violations.push({ path, message: `${value} > maximum ${maximum}` });
    }
  }
  if (Array.isArray(value)) {
    const items = schemaAt(schema, "items");
    if (items !== undefined) {
      for (const [index, item] of value.entries()) {
        violations.push(
          ...validateJsonSchema(item, items, `${path}[${index}]`)
        );
      }
    }
    return violations;
  }
  if (isRecord(value)) {
    const properties = schemaAt(schema, "properties") ?? {};
    const required = schema["required"];
    if (Array.isArray(required)) {
      for (const key of required) {
        if (typeof key === "string" && !(key in value)) {
          violations.push({
            path: `${path}.${key}`,
            message: "required property missing",
          });
        }
      }
    }
    for (const [key, child] of Object.entries(value)) {
      const childSchema = properties[key];
      if (isRecord(childSchema)) {
        violations.push(
          ...validateJsonSchema(child, childSchema, `${path}.${key}`)
        );
      } else if (schema["additionalProperties"] === false) {
        violations.push({
          path: `${path}.${key}`,
          message: "unexpected property",
        });
      }
    }
  }
  return violations;
}
