import { isRecord } from "../../internal/guards";
import type { SchemaViolation, Validate } from "./json-schema-collections";
import {
  arrayViolations,
  jsonEqual,
  objectViolations,
} from "./json-schema-collections";

export type JsonSchema = Readonly<Record<string, unknown>>;

interface SchemaLocation {
  readonly schema: JsonSchema;
  readonly path: string;
  readonly root: JsonSchema;
}

export function resolveRef(
  ref: string,
  root: JsonSchema
): JsonSchema | undefined {
  if (!ref.startsWith("#/")) {
    return undefined;
  }
  let current: unknown = root;
  for (const segment of ref.slice(2).split("/")) {
    if (!isRecord(current)) {
      return undefined;
    }
    current =
      current[
        decodeURIComponent(segment).replaceAll("~1", "/").replaceAll("~0", "~")
      ];
  }
  return isRecord(current) ? current : undefined;
}

function countMatches(
  value: unknown,
  {
    branches,
    location,
  }: {
    readonly branches: readonly unknown[];
    readonly location: SchemaLocation;
  }
): number {
  return branches
    .filter(isRecord)
    .filter(
      (branch) =>
        validateAt(value, { ...location, schema: branch }).length === 0
    ).length;
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

interface Bound {
  readonly keyword: string;
  readonly fails: (value: number, limit: number) => boolean;
}

const NUMBER_BOUNDS: readonly Bound[] = [
  { keyword: "minimum", fails: (value, limit) => value < limit },
  { keyword: "maximum", fails: (value, limit) => value > limit },
  { keyword: "exclusiveMinimum", fails: (value, limit) => value <= limit },
  { keyword: "exclusiveMaximum", fails: (value, limit) => value >= limit },
  {
    keyword: "multipleOf",
    fails: (value, limit) => !Number.isInteger(value / limit),
  },
];

function boundsOf({
  value,
  bounds,
  location,
}: {
  readonly value: number;
  readonly bounds: readonly Bound[];
  readonly location: SchemaLocation;
}): SchemaViolation[] {
  return bounds.flatMap(({ keyword, fails }) => {
    const limit = location.schema[keyword];
    return typeof limit === "number" && fails(value, limit)
      ? [
          {
            path: location.path,
            message: `${value} violates ${keyword} ${limit}`,
          },
        ]
      : [];
  });
}

const STRING_BOUNDS: readonly Bound[] = [
  { keyword: "minLength", fails: (length, limit) => length < limit },
  { keyword: "maxLength", fails: (length, limit) => length > limit },
];

const ARRAY_BOUNDS: readonly Bound[] = [
  { keyword: "minItems", fails: (length, limit) => length < limit },
  { keyword: "maxItems", fails: (length, limit) => length > limit },
];

function stringViolations(
  value: string,
  location: SchemaLocation
): SchemaViolation[] {
  const pattern = location.schema["pattern"];
  const mismatch =
    typeof pattern === "string"
      ? patternViolations({ value, pattern, path: location.path })
      : [];
  return [
    ...boundsOf({ value: [...value].length, bounds: STRING_BOUNDS, location }),
    ...mismatch,
  ];
}

function patternViolations({
  value,
  pattern,
  path,
}: {
  readonly value: string;
  readonly pattern: string;
  readonly path: string;
}): SchemaViolation[] {
  return new RegExp(pattern, "u").test(value)
    ? []
    : [
        {
          path,
          message: `string ${JSON.stringify(value)} does not match ${pattern}`,
        },
      ];
}

function validateAt(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const applied = APPLICATORS.flatMap((check) => check(value, location));
  return [...applied, ...assertionViolations(value, location)];
}

function refViolations(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const ref = location.schema["$ref"];
  if (typeof ref !== "string") {
    return [];
  }
  const target = resolveRef(ref, location.root);
  return target === undefined
    ? [{ path: location.path, message: `unresolvable $ref ${ref}` }]
    : validateAt(value, { ...location, schema: target });
}

function unionViolations(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const { schema, path } = location;
  const anyOf = schema["anyOf"];
  const anyOfFails =
    Array.isArray(anyOf) &&
    countMatches(value, { branches: anyOf, location }) === 0;
  const oneOf = schema["oneOf"];
  const oneOfMatches = Array.isArray(oneOf)
    ? countMatches(value, { branches: oneOf, location })
    : 1;
  return [
    ...(anyOfFails
      ? [{ path, message: `${describeValue(value)} matched no anyOf branch` }]
      : []),
    ...(oneOfMatches === 1
      ? []
      : [
          {
            path,
            message: `${describeValue(value)} matched ${oneOfMatches} oneOf branches`,
          },
        ]),
  ];
}

function allOfViolations(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const allOf = location.schema["allOf"];
  return Array.isArray(allOf)
    ? allOf
        .filter(isRecord)
        .flatMap((branch) => validateAt(value, { ...location, schema: branch }))
    : [];
}

function notViolations(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const negated = schemaAt(location.schema, "not");
  return negated !== undefined &&
    validateAt(value, { ...location, schema: negated }).length === 0
    ? [
        {
          path: location.path,
          message: `${describeValue(value)} matched the not schema`,
        },
      ]
    : [];
}

function conditionalViolations(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const condition = schemaAt(location.schema, "if");
  if (condition === undefined) {
    return [];
  }
  const holds =
    validateAt(value, { ...location, schema: condition }).length === 0;
  const branch = schemaAt(location.schema, holds ? "then" : "else");
  return branch === undefined
    ? []
    : validateAt(value, { ...location, schema: branch });
}

const APPLICATORS: readonly ((
  value: unknown,
  location: SchemaLocation
) => readonly SchemaViolation[])[] = [
  refViolations,
  unionViolations,
  allOfViolations,
  notViolations,
  conditionalViolations,
];

function assertionViolations(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  const { schema, path } = location;
  if ("const" in schema && !jsonEqual(schema["const"], value)) {
    return [
      {
        path,
        message: `${describeValue(value)} is not ${JSON.stringify(schema["const"])}`,
      },
    ];
  }
  const type = schema["type"];
  const types = Array.isArray(type) ? type : [type];
  if (
    type !== undefined &&
    !types.some((candidate) => matchesType(value, candidate))
  ) {
    return [
      {
        path,
        message: `expected ${types.join(" | ")}, got ${describeValue(value)}`,
      },
    ];
  }
  const enumValues = schema["enum"];
  if (
    Array.isArray(enumValues) &&
    !enumValues.some((option) => jsonEqual(option, value))
  ) {
    return [
      {
        path,
        message: `${describeValue(value)} is not one of ${JSON.stringify(enumValues)}`,
      },
    ];
  }
  return typedViolations(value, location);
}

function typedViolations(
  value: unknown,
  location: SchemaLocation
): readonly SchemaViolation[] {
  if (typeof value === "number") {
    return boundsOf({ value, bounds: NUMBER_BOUNDS, location });
  }
  if (typeof value === "string") {
    return stringViolations(value, location);
  }
  const validate: Validate = (child, at) =>
    validateAt(child, { ...location, ...at });
  if (Array.isArray(value)) {
    return [
      ...boundsOf({ value: value.length, bounds: ARRAY_BOUNDS, location }),
      ...arrayViolations(value, {
        schema: location.schema,
        path: location.path,
        validate,
      }),
    ];
  }
  return isRecord(value)
    ? objectViolations(value, {
        schema: location.schema,
        path: location.path,
        validate,
      })
    : [];
}

export function validateJsonSchema(
  value: unknown,
  schema: JsonSchema
): readonly SchemaViolation[] {
  return validateAt(value, { schema, path: "$", root: schema });
}
