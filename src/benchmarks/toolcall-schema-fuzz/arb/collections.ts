import type { Arbitrary } from "fast-check";
import {
  array,
  boolean,
  constant,
  constantFrom,
  integer,
  nat,
  oneof,
  option,
  record,
  subarray,
  tuple,
} from "fast-check";

import type { ValueOf } from "../../../internal/guards";
import type { JsonSchema } from "../json-schema";
import { validateJsonSchema } from "../json-schema";
import type { Context, SchemaNode } from "./node";
import { accepts, deeper, withChildren } from "./node";
import { jsonValues, keys } from "./values";

const Placement = {
  Property: "property",
  Pattern: "pattern",
  Additional: "additional",
} as const;
type Placement = ValueOf<typeof Placement>;

const MAX_ABSENT_PROPERTIES = 2;

function escapeRegex(text: string): string {
  return text.replaceAll(/[$()*+./?[\\\]^{|}]/g, String.raw`\$&`);
}

export function anyOfNode(nodes: readonly SchemaNode[]): SchemaNode {
  return withChildren({ anyOf: nodes.map((node) => node.schema) }, nodes);
}

function schemaForAll(
  values: readonly unknown[],
  context: Context
): Arbitrary<SchemaNode> {
  if (values.length === 0) {
    return jsonValues.chain((other) => context.describe(other, context));
  }
  const [first] = values;
  return context
    .describe(first, context)
    .chain((node) =>
      values.every((value) => accepts(value, node))
        ? constant(node)
        : tuple(...values.map((value) => context.describe(value, context))).map(
            anyOfNode
          )
    );
}

function matchCount(values: readonly unknown[], node: SchemaNode): number {
  return values.filter((value) => accepts(value, node)).length;
}

function containsKeywords(
  values: readonly unknown[],
  context: Context
): Arbitrary<SchemaNode> {
  return constantFrom(...values.keys())
    .chain((index) => context.describe(values[index], context))
    .chain((node) => {
      const count = matchCount(values, node);
      return record(
        {
          minContains: integer({ min: 1, max: count }),
          maxContains: integer({ min: count, max: count + 2 }),
        },
        { requiredKeys: [] }
      ).map((bounds) =>
        withChildren({ contains: node.schema, ...bounds }, [node])
      );
    });
}

function itemsNode(
  values: readonly unknown[],
  context: Context
): Arbitrary<SchemaNode> {
  const uniform = schemaForAll(values, context).map((node) =>
    withChildren({ items: node.schema }, [node])
  );
  if (context.strict || values.length === 0) {
    return uniform;
  }
  const prefixed = integer({ min: 1, max: values.length }).chain((length) =>
    tuple(
      tuple(
        ...values
          .slice(0, length)
          .map((value) => context.describe(value, context))
      ),
      length === values.length
        ? constantFrom<SchemaNode | false | undefined>(false, undefined)
        : schemaForAll(values.slice(length), context)
    ).map(([prefix, rest]) =>
      withChildren(
        {
          prefixItems: prefix.map((node) => node.schema),
          ...(rest === undefined
            ? {}
            : { items: rest === false ? false : rest.schema }),
        },
        [...prefix, ...(rest === undefined || rest === false ? [] : [rest])]
      )
    )
  );
  return oneof({ arbitrary: uniform, weight: 2 }, prefixed);
}

function isUnique(values: readonly unknown[]): boolean {
  return validateJsonSchema(values, { uniqueItems: true }).length === 0;
}

export function arrayNode(
  values: readonly unknown[],
  context: Context
): Arbitrary<SchemaNode> {
  const child = deeper(context);
  const loose = !context.strict;
  return tuple(
    itemsNode(values, child),
    record(
      {
        minItems: integer({ min: 0, max: values.length }),
        maxItems: integer({ min: values.length, max: values.length + 3 }),
        ...(loose && isUnique(values) ? { uniqueItems: constant(true) } : {}),
      },
      { requiredKeys: [] }
    ),
    loose && values.length > 0
      ? option(containsKeywords(values, child), { nil: undefined })
      : constant(undefined)
  ).map(([items, bounds, contains]) =>
    withChildren(
      { type: "array", ...items.schema, ...bounds, ...contains?.schema },
      [items, ...(contains === undefined ? [] : [contains])]
    )
  );
}

function placements(
  count: number,
  strict: boolean
): Arbitrary<readonly Placement[]> {
  const placement: Arbitrary<Placement> = strict
    ? constant(Placement.Property)
    : oneof(
        { arbitrary: constant(Placement.Property), weight: 6 },
        constant(Placement.Pattern),
        constant(Placement.Additional)
      );
  return array(placement, { minLength: count, maxLength: count });
}

function describeEach(
  {
    value,
    names,
  }: {
    readonly value: Readonly<Record<string, unknown>>;
    readonly names: readonly string[];
  },
  context: Context
): Arbitrary<readonly (readonly [string, SchemaNode])[]> {
  return tuple(
    ...names.map((name) =>
      context
        .describe(value[name], context)
        .map((node) => [name, node] as const)
    )
  );
}

function additionalNode(
  values: readonly unknown[],
  context: Context
): Arbitrary<SchemaNode | boolean | undefined> {
  if (context.strict) {
    return constant(false);
  }
  if (values.length > 0) {
    return oneof(
      schemaForAll(values, context),
      constant(undefined),
      constant(true)
    );
  }
  return oneof(
    constant<boolean | undefined>(false),
    constant(undefined),
    constant(true),
    jsonValues.chain((other) => context.describe(other, context))
  );
}

function absentProperties(
  value: Readonly<Record<string, unknown>>,
  context: Context
): Arbitrary<readonly (readonly [string, SchemaNode])[]> {
  if (context.strict) {
    return constant([]);
  }
  const absent = keys.filter((key) => !(key in value));
  return array(
    tuple(
      absent,
      jsonValues.chain((other) => context.describe(other, deeper(context)))
    ),
    { maxLength: MAX_ABSENT_PROPERTIES }
  );
}

function objectExtras(
  value: Readonly<Record<string, unknown>>,
  context: Context
): Arbitrary<JsonSchema> {
  if (context.strict) {
    return constant({});
  }
  const names = Object.keys(value);
  const count = names.length;
  const longest = Math.max(0, ...names.map((name) => [...name].length));
  const pair =
    count >= 2
      ? constantFrom(...names).chain((trigger) =>
          constantFrom(...names.filter((name) => name !== trigger)).map(
            (other) => [trigger, other] as const
          )
        )
      : undefined;
  return record(
    {
      minProperties: integer({ min: 0, max: count }),
      maxProperties: integer({ min: count, max: count + 3 }),
      propertyNames: oneof(
        nat(4).map((slack) => ({ maxLength: longest + slack })),
        constant({ type: "string" }),
        ...(names.every((name) => name.length > 0)
          ? [constant({ minLength: 1 })]
          : [])
      ),
      ...(pair === undefined
        ? {}
        : {
            dependentRequired: pair.map(([trigger, other]) => ({
              [trigger]: [other],
            })),
            dependentSchemas: pair.map(([trigger, other]) => ({
              [trigger]: { required: [other] },
            })),
          }),
    },
    { requiredKeys: [] }
  );
}

interface ObjectParts {
  readonly properties: readonly (readonly [string, SchemaNode])[];
  readonly patterned: readonly (readonly [string, SchemaNode])[];
  readonly additional: SchemaNode | boolean | undefined;
  readonly absent: readonly (readonly [string, SchemaNode])[];
  readonly required: readonly string[];
  readonly extras: JsonSchema;
  readonly typed: boolean;
}

function buildObject(parts: ObjectParts): SchemaNode {
  const properties = [...parts.properties, ...parts.absent];
  const additional = parts.additional;
  const children = [
    ...properties.map(([, node]) => node),
    ...parts.patterned.map(([, node]) => node),
    ...(additional === undefined || typeof additional === "boolean"
      ? []
      : [additional]),
  ];
  return withChildren(
    {
      ...(parts.typed ? { type: "object" } : {}),
      properties: Object.fromEntries(
        properties.map(([name, node]) => [name, node.schema])
      ),
      required: parts.required,
      ...(parts.patterned.length > 0
        ? {
            patternProperties: Object.fromEntries(
              parts.patterned.map(([name, node]) => [
                `^${escapeRegex(name)}$`,
                node.schema,
              ])
            ),
          }
        : {}),
      ...(additional === undefined
        ? {}
        : {
            additionalProperties:
              typeof additional === "boolean" ? additional : additional.schema,
          }),
      ...parts.extras,
    },
    children
  );
}

export function objectNode(
  value: Readonly<Record<string, unknown>>,
  context: Context
): Arbitrary<SchemaNode> {
  const names = Object.keys(value);
  const child = deeper(context);
  return placements(names.length, context.strict).chain((placed) => {
    const named = (placement: Placement): readonly string[] =>
      names.filter((_, index) => placed[index] === placement);
    const propertyNames = named(Placement.Property);
    return record({
      properties: describeEach({ value, names: propertyNames }, child),
      patterned: describeEach(
        { value, names: named(Placement.Pattern) },
        child
      ),
      additional: additionalNode(
        named(Placement.Additional).map((name) => value[name]),
        child
      ),
      absent: absentProperties(value, child),
      required: context.strict
        ? constant(propertyNames)
        : subarray([...propertyNames]),
      extras: objectExtras(value, context),
      typed: context.strict
        ? constant(true)
        : oneof({ arbitrary: constant(true), weight: 4 }, boolean()),
    }).map(buildObject);
  });
}
