import { definedValues, isRecord } from "../../internal/guards";
import { pyRepr, pyTypeName } from "./py-format";
import type { JsonSchemaNode, TopLevelCount } from "./schema";

export interface FieldCheck {
  readonly path: string;
  readonly passed: boolean;
  readonly error?: string;
}

export interface TopLevelStructure {
  readonly data: unknown;
  readonly wasWrapped: boolean;
  readonly error?: string;
}

export function validateAgainstJsonSchema(
  data: unknown,
  schema: JsonSchemaNode,
  path = ""
): FieldCheck[] {
  const checks: FieldCheck[] = [];
  const rootPath = path || "root";
  const rawType = schema.type;
  if (Array.isArray(rawType) && data === null) {
    if (rawType.includes("null")) {
      return [{ path: rootPath, passed: true }];
    }
    return [
      {
        path: rootPath,
        passed: false,
        error: `expected ${pyRepr(rawType)}, got NoneType`,
      },
    ];
  }
  const schemaType = normalizeSchemaType(rawType);
  if (data === null && schemaType !== "null") {
    return [
      {
        path: rootPath,
        passed: false,
        error: `expected ${describeSchemaType(schemaType)}, got NoneType`,
      },
    ];
  }
  switch (schemaType) {
    case "array": {
      if (!Array.isArray(data)) {
        return [
          {
            path: rootPath,
            passed: false,
            error: `expected array, got ${pyTypeName(data)}`,
          },
        ];
      }
      const minItems = schema.minItems;
      const maxItems = schema.maxItems;
      if (minItems !== undefined && data.length < minItems) {
        checks.push({
          path: rootPath,
          passed: false,
          error: `array has ${data.length} items, minimum is ${minItems}`,
        });
      } else if (maxItems !== undefined && data.length > maxItems) {
        checks.push({
          path: rootPath,
          passed: false,
          error: `array has ${data.length} items, maximum is ${maxItems}`,
        });
      }
      const itemsSchema = schema.items;
      if (itemsSchema !== undefined) {
        data.forEach((item, i) => {
          const itemPath = path ? `${path}[${i}]` : `[${i}]`;
          checks.push(
            ...validateAgainstJsonSchema(item, itemsSchema, itemPath)
          );
        });
      }
      return checks;
    }
    case "object": {
      if (!isRecord(data)) {
        return [
          {
            path: rootPath,
            passed: false,
            error: `expected object, got ${pyTypeName(data)}`,
          },
        ];
      }
      const required = schema.required ?? [];
      const properties = schema.properties ?? {};
      for (const fieldName of required) {
        const fieldPath = path ? `${path}.${fieldName}` : fieldName;
        if (!(fieldName in data)) {
          const propSchema = properties[fieldName];
          const leafCount =
            propSchema !== undefined ? countSchemaLeaves(propSchema) : 1;
          for (let i = 0; i < leafCount; i++) {
            checks.push({
              path: fieldPath,
              passed: false,
              error: "required field missing",
            });
          }
        }
      }
      for (const [fieldName, fieldSchema] of Object.entries(properties)) {
        if (fieldName in data) {
          const fieldPath = path ? `${path}.${fieldName}` : fieldName;
          checks.push(
            ...validateAgainstJsonSchema(
              data[fieldName],
              fieldSchema,
              fieldPath
            )
          );
        }
      }
      const extraKeys = Object.keys(data)
        .filter((key) => !(key in properties))
        .sort();
      for (const key of extraKeys) {
        const fieldPath = path ? `${path}.${key}` : key;
        checks.push({
          path: fieldPath,
          passed: false,
          error: `extraneous field '${key}'`,
        });
      }
      return checks;
    }
    case "string": {
      const ok = typeof data === "string";
      checks.push(
        definedValues({
          path: rootPath,
          passed: ok,
          error: ok ? undefined : `expected string, got ${pyTypeName(data)}`,
        })
      );
      break;
    }
    case "number": {
      if (typeof data !== "number") {
        checks.push({
          path: rootPath,
          passed: false,
          error: `expected number, got ${pyTypeName(data)}`,
        });
      } else {
        checks.push(rangeCheck(rootPath, data, schema));
      }
      break;
    }
    case "integer": {
      if (typeof data !== "number" || !Number.isInteger(data)) {
        checks.push({
          path: rootPath,
          passed: false,
          error: `expected integer, got ${pyTypeName(data)}`,
        });
      } else {
        checks.push(rangeCheck(rootPath, data, schema));
      }
      break;
    }
    case "boolean": {
      const ok = typeof data === "boolean";
      checks.push(
        definedValues({
          path: rootPath,
          passed: ok,
          error: ok ? undefined : `expected boolean, got ${pyTypeName(data)}`,
        })
      );
      break;
    }
    default: {
      break;
    }
  }
  const enumValues = schema.enum;
  if (enumValues !== undefined) {
    const ok = enumValues.some((v) => v === data);
    checks.push(
      definedValues({
        path: rootPath,
        passed: ok,
        error: !ok
          ? `${pyRepr(data)} not in allowed values ${pyRepr(enumValues)}`
          : undefined,
      })
    );
  }
  return checks;
}

function normalizeSchemaType(
  schemaType: string | readonly string[] | undefined
): string | readonly string[] | undefined {
  if (!Array.isArray(schemaType)) {
    return schemaType;
  }
  const nonNull = schemaType.filter((t) => t !== "null");
  return nonNull.length === 1 ? nonNull[0] : schemaType;
}

function describeSchemaType(
  schemaType: string | readonly string[] | undefined
): string {
  if (Array.isArray(schemaType)) {
    return pyRepr(schemaType);
  }
  return String(schemaType);
}

function rangeCheck(
  path: string,
  data: number,
  schema: JsonSchemaNode
): FieldCheck {
  const minimum = schema.minimum;
  const maximum = schema.maximum;
  if (minimum !== undefined && data < minimum) {
    return {
      path,
      passed: false,
      error: `${data} is less than minimum ${minimum}`,
    };
  }
  if (maximum !== undefined && data > maximum) {
    return {
      path,
      passed: false,
      error: `${data} is greater than maximum ${maximum}`,
    };
  }
  return { path, passed: true };
}

export function countSchemaLeaves(schema: JsonSchemaNode): number {
  const schemaType = normalizeSchemaType(schema.type);
  if (schemaType === "object") {
    const properties = schema.properties ?? {};
    const propValues = Object.values(properties);
    if (propValues.length === 0) {
      return 1;
    }
    const required = schema.required;
    const selected: JsonSchemaNode[] =
      required === undefined
        ? propValues
        : required
            .map((name) => properties[name])
            .filter((s): s is JsonSchemaNode => s !== undefined);
    return selected.length > 0
      ? selected.reduce((sum, s) => sum + countSchemaLeaves(s), 0)
      : 1;
  }
  if (schemaType === "array") {
    const itemsSchema = schema.items;
    return itemsSchema !== undefined ? countSchemaLeaves(itemsSchema) : 1;
  }
  let count = 1;
  if (schema.enum !== undefined) {
    count += 1;
  }
  return count;
}

export function computeExpectedChecks(
  jsonSchema: JsonSchemaNode,
  topLevelCount: TopLevelCount
): number {
  if (jsonSchema.type === "array") {
    const itemsSchema = jsonSchema.items;
    if (itemsSchema === undefined) {
      return 0;
    }
    const leavesPerItem = countSchemaLeaves(itemsSchema);
    if (typeof topLevelCount === "number") {
      return topLevelCount * leavesPerItem;
    }
    if (Array.isArray(topLevelCount)) {
      return topLevelCount[0] * leavesPerItem;
    }
    return 0;
  }
  return countSchemaLeaves(jsonSchema);
}

export function checkTopLevelCount(
  data: unknown,
  topLevelCount: TopLevelCount
): string | null {
  if (topLevelCount === null || !Array.isArray(data)) {
    return null;
  }
  const actual = data.length;
  if (typeof topLevelCount === "number") {
    return actual !== topLevelCount
      ? `Expected ${topLevelCount} items, got ${actual}`
      : null;
  }
  const [minCount, maxCount] = topLevelCount;
  if (actual < minCount || actual > maxCount) {
    return `Expected ${minCount}-${maxCount} items, got ${actual}`;
  }
  return null;
}

export function checkTopLevelStructure(
  data: unknown,
  expectedKey: string | null,
  requireWrapper: boolean
): TopLevelStructure {
  if (Array.isArray(data)) {
    if (requireWrapper) {
      return {
        data,
        wasWrapped: false,
        error: `Expected wrapped object with key '${expectedKey}', got bare list`,
      };
    }
    return { data, wasWrapped: false };
  }
  if (isRecord(data) && Object.keys(data).length === 1) {
    const onlyKey = Object.keys(data)[0] ?? "";
    const onlyValue = data[onlyKey];
    if (Array.isArray(onlyValue)) {
      if (!requireWrapper) {
        return {
          data: onlyValue,
          wasWrapped: true,
          error: `Expected bare list, got wrapped object with key '${onlyKey}'`,
        };
      }
      if (expectedKey === null || onlyKey === expectedKey) {
        return { data: onlyValue, wasWrapped: true };
      }
      return {
        data: onlyValue,
        wasWrapped: true,
        error: `Expected top-level key '${expectedKey}', got '${onlyKey}'`,
      };
    }
  }
  return { data, wasWrapped: false };
}
